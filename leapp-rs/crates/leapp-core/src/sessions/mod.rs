//! Start and stop of sessions (spec 10.1, 10.2).

mod iam_user;

use uuid::Uuid;

use crate::Leapp;
use crate::aws_files::CredentialsFile;
use crate::error::{Error, Result};
use crate::model::{ManagedProfile, SessionId, SessionKind, SessionStatus, Workspace};
use crate::secrets::SecretStore;
use crate::store;

impl Leapp {
    /// Starts a session: stops other sessions on the same profile, gets credentials, writes the
    /// profile section. On error the status is `Inactive` and the credentials file is unchanged.
    pub async fn start(&self, id: SessionId) -> Result<()> {
        let (profile_id, profile, to_stop) = {
            let ws = self.workspace.lock().unwrap();
            let session = ws.session(id).ok_or(Error::NotFound(id))?;
            let profile = profile_name(&ws, session.profile_id)?;
            if pending_on(&ws, session.profile_id) {
                return Err(pending_error());
            }
            let to_stop: Vec<SessionId> = ws
                .sessions
                .iter()
                .filter(|s| s.profile_id == session.profile_id && s.status == SessionStatus::Active)
                .map(|s| s.id)
                .collect();
            (session.profile_id, profile, to_stop)
        };
        for other in to_stop {
            self.stop(other).await?;
        }
        {
            let mut ws = self.workspace.lock().unwrap();
            if pending_on(&ws, profile_id) {
                return Err(pending_error());
            }
            set_status(&mut ws, id, SessionStatus::Pending);
        }

        let result = self.activate(id, &profile).await;
        if result.is_err() {
            let mut ws = self.workspace.lock().unwrap();
            set_status(&mut ws, id, SessionStatus::Inactive);
        }
        result
    }

    /// Stops a session: puts its profile section back as it was (the user's saved section, or
    /// none). Does nothing if it is `Inactive`. A `Pending` session cannot be stopped.
    pub async fn stop(&self, id: SessionId) -> Result<()> {
        let mut ws = self.workspace.lock().unwrap();
        let session = ws.session(id).ok_or(Error::NotFound(id))?;
        match session.status {
            SessionStatus::Inactive => return Ok(()),
            SessionStatus::Pending => return Err(pending_error()),
            SessionStatus::Active => {}
        }
        let profile = profile_name(&ws, session.profile_id)?;
        let file = CredentialsFile::new(&self.config.home_dir);

        let mut next = ws.clone();
        let released = self.release_profile(&mut next, &file, &profile, id)?;
        set_inactive(&mut next, id);
        if let Err(e) = store::save(&self.workspace_path, &next) {
            released.rollback(&file);
            return Err(e);
        }
        *ws = next;
        released.finish(self.config.secrets.as_ref());
        Ok(())
    }

    /// Cleans up after a crash: every managed profile is restored or removed, every session is
    /// set to `Inactive`.
    pub async fn startup(&self) -> Result<()> {
        self.clean_all()
    }

    /// Same cleanup as `startup`, for a normal exit.
    pub async fn shutdown(&self) -> Result<()> {
        self.clean_all()
    }

    fn clean_all(&self) -> Result<()> {
        let mut ws = self.workspace.lock().unwrap();
        let file = CredentialsFile::new(&self.config.home_dir);
        let mut next = ws.clone();
        let managed: Vec<(String, SessionId)> = next
            .managed_profiles
            .iter()
            .map(|(profile, m)| (profile.clone(), m.session_id))
            .collect();
        let mut released = Vec::new();
        for (profile, owner) in managed {
            match self.release_profile(&mut next, &file, &profile, owner) {
                Ok(r) => released.push(r),
                Err(e) => {
                    rollback_all(&file, &released);
                    return Err(e);
                }
            }
        }
        for s in &mut next.sessions {
            s.status = SessionStatus::Inactive;
            s.start_time = None;
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            rollback_all(&file, &released);
            return Err(e);
        }
        *ws = next;
        for r in &released {
            r.finish(self.config.secrets.as_ref());
        }
        Ok(())
    }

    /// Spec 9.3 step 6 for one profile: if `owner` manages it, put the user's saved section back
    /// (or remove ours) and drop the entry from `next`. The secret backup is not deleted here;
    /// `Released::finish` does that after the workspace is saved.
    fn release_profile(
        &self,
        next: &mut Workspace,
        file: &CredentialsFile,
        profile: &str,
        owner: SessionId,
    ) -> Result<Released> {
        let mut released = Released {
            profile: profile.to_string(),
            previous_section: None,
            backup_key: None,
            changed: false,
        };
        let Some(managed) = next.managed_profiles.get(profile) else {
            return Ok(released);
        };
        if managed.session_id != owner {
            return Ok(released);
        }
        let had_original = managed.had_original;
        released.previous_section = file.section_text(profile)?;
        let backup = if had_original {
            let key = backup_key(profile);
            let saved = self.config.secrets.get(&key)?;
            released.backup_key = Some(key);
            saved
        } else {
            None
        };
        match backup {
            Some(text) => file.write_raw_section(profile, &text)?,
            None => file.remove_section(profile)?,
        }
        released.changed = true;
        next.managed_profiles.remove(profile);
        Ok(released)
    }

    /// Steps 3 to 7 of spec 10.1 and 9.3. Changes the file, the workspace and the secret store
    /// together or not at all (except the backup of the user's section, which is deleted again
    /// on failure).
    async fn activate(&self, id: SessionId, profile: &str) -> Result<()> {
        let (session, existing) = {
            let ws = self.workspace.lock().unwrap();
            (
                ws.session(id).ok_or(Error::NotFound(id))?.clone(),
                ws.managed_profiles.get(profile).cloned(),
            )
        };
        let file = CredentialsFile::new(&self.config.home_dir);
        let secrets = self.config.secrets.as_ref();

        let managed_already = existing.is_some();
        let mut had_original = existing.is_some_and(|m| m.had_original);
        let mut new_backup = None;
        if !managed_already {
            match file.sections_named(profile)? {
                0 => {}
                1 => {
                    if !self
                        .config
                        .prompter
                        .confirm_profile_overwrite(profile)
                        .await
                    {
                        return Err(Error::ProfileOverwriteDeclined(profile.to_string()));
                    }
                    if let Some(text) = file.section_text(profile)? {
                        let key = backup_key(profile);
                        secrets.set(&key, &text)?;
                        new_backup = Some(key);
                        had_original = true;
                    }
                }
                _ => return Err(Error::DuplicateProfileSection(profile.to_string())),
            }
        }

        let result = self
            .write_active(id, profile, &session, &file, had_original)
            .await;
        if result.is_err()
            && let Some(key) = new_backup
        {
            let _ = secrets.delete(&key);
        }
        result
    }

    async fn write_active(
        &self,
        id: SessionId,
        profile: &str,
        session: &crate::model::Session,
        file: &CredentialsFile,
        had_original: bool,
    ) -> Result<()> {
        let (body, expiration) = match session.kind {
            SessionKind::IamUser { .. } => self.iam_user_credentials(session).await?,
        };

        let mut ws = self.workspace.lock().unwrap();
        let previous_section = file.section_text(profile)?;
        file.write_section(profile, &body)?;

        let mut next = ws.clone();
        next.managed_profiles.insert(
            profile.to_string(),
            ManagedProfile {
                session_id: id,
                had_original,
            },
        );
        if let Some(s) = next.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Active;
            s.start_time = Some(self.config.clock.now());
            s.session_token_expiration = Some(expiration);
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            restore(file, profile, previous_section);
            return Err(e);
        }
        *ws = next;
        Ok(())
    }
}

/// What `release_profile` changed, so that a failed save can undo it.
struct Released {
    profile: String,
    previous_section: Option<String>,
    backup_key: Option<String>,
    changed: bool,
}

impl Released {
    fn rollback(&self, file: &CredentialsFile) {
        if self.changed {
            restore(file, &self.profile, self.previous_section.clone());
        }
    }

    /// Deletes the backup. The workspace is already saved, so an error here is dropped; the
    /// next backup of the same profile overwrites the key.
    fn finish(&self, secrets: &dyn SecretStore) {
        if let Some(key) = &self.backup_key {
            let _ = secrets.delete(key);
        }
    }
}

fn rollback_all(file: &CredentialsFile, released: &[Released]) {
    for r in released.iter().rev() {
        r.rollback(file);
    }
}

fn backup_key(profile: &str) -> String {
    format!("aws-profile-backup-{profile}")
}

fn set_inactive(ws: &mut Workspace, id: SessionId) {
    if let Some(s) = ws.sessions.iter_mut().find(|s| s.id == id) {
        s.status = SessionStatus::Inactive;
        s.start_time = None;
    }
}

fn profile_name(ws: &Workspace, profile_id: Uuid) -> Result<String> {
    ws.profile_name(profile_id)
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidInput {
            field: "profile",
            message: "the profile of the session does not exist".into(),
        })
}

fn pending_on(ws: &Workspace, profile_id: Uuid) -> bool {
    ws.sessions
        .iter()
        .any(|s| s.profile_id == profile_id && s.status == SessionStatus::Pending)
}

fn pending_error() -> Error {
    Error::ProfilePending("Another session with the same named profile is starting".into())
}

fn set_status(ws: &mut Workspace, id: SessionId, status: SessionStatus) {
    if let Some(s) = ws.sessions.iter_mut().find(|s| s.id == id) {
        s.status = status;
    }
}

/// Puts the section back as it was, after a failed save. The error of the save is more important
/// than an error here, so this one is dropped.
fn restore(file: &CredentialsFile, profile: &str, previous_section: Option<String>) {
    let _ = match previous_section {
        Some(text) => file.write_raw_section(profile, &text),
        None => file.remove_section(profile),
    };
}
