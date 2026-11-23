//! Start and stop of sessions (spec 10.1, 10.2).

mod iam_user;

use std::sync::atomic::Ordering;

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
    /// After `shutdown` began, it returns `Error::Closing` and changes nothing.
    ///
    /// This function does blocking file and keychain I/O while it holds the workspace lock. Call
    /// it from a task that may block (the UI calls it from its tokio bridge, not the render
    /// thread).
    pub async fn start(&self, id: SessionId) -> Result<()> {
        let (profile_id, profile, to_stop, consent_carried) = {
            let ws = self.workspace.lock().unwrap();
            if self.closing.load(Ordering::SeqCst) {
                return Err(Error::Closing);
            }
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
            // Spec 9.3 step 5: the user agreed to the overwrite of this profile when the session
            // we stop now started. That consent carries over to this start: no second question.
            let consent_carried = ws
                .managed_profiles
                .get(&profile)
                .is_some_and(|m| m.had_original && to_stop.contains(&m.session_id));
            (session.profile_id, profile, to_stop, consent_carried)
        };
        for other in to_stop {
            self.stop(other).await?;
        }
        {
            let mut ws = self.workspace.lock().unwrap();
            if self.closing.load(Ordering::SeqCst) {
                return Err(Error::Closing);
            }
            if pending_on(&ws, profile_id) {
                return Err(pending_error());
            }
            set_status(&mut ws, id, SessionStatus::Pending);
        }

        let result = self.activate(id, &profile, consent_carried).await;
        if result.is_err() {
            let mut ws = self.workspace.lock().unwrap();
            set_status(&mut ws, id, SessionStatus::Inactive);
        }
        result
    }

    /// Stops a session: puts its profile section back as it was (the user's saved section, or
    /// none). Does nothing if it is `Inactive`. A `Pending` session cannot be stopped.
    ///
    /// This function does blocking file and keychain I/O while it holds the workspace lock. Call
    /// it from a task that may block (the UI calls it from its tokio bridge, not the render
    /// thread).
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
    ///
    /// This function does blocking file and keychain I/O while it holds the workspace lock. Call
    /// it from a task that may block (the UI calls it from its tokio bridge, not the render
    /// thread).
    pub async fn startup(&self) -> Result<()> {
        self.clean_all(false)
    }

    /// Same cleanup as `startup`, for a normal exit. It is final: a start that is still in
    /// flight (for example one that waits on AWS) fails with `Error::Closing` before it writes,
    /// and later starts fail the same way.
    ///
    /// This function does blocking file and keychain I/O while it holds the workspace lock. Call
    /// it from a task that may block (the UI calls it from its tokio bridge, not the render
    /// thread).
    pub async fn shutdown(&self) -> Result<()> {
        self.clean_all(true)
    }

    /// With `closing`, sets the closing flag under the lock first, so no start can write after
    /// the cleanup.
    fn clean_all(&self, closing: bool) -> Result<()> {
        let mut ws = self.workspace.lock().unwrap();
        if closing {
            self.closing.store(true, Ordering::SeqCst);
        }
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

    /// Steps 3 to 7 of spec 10.1 and 9.3. Changes the file and the workspace together or not at
    /// all. The backup of the user's section is deleted on failure only when the file and the
    /// workspace are back as before; else it stays, because it is the only copy.
    ///
    /// With `consent_carried`, the user already agreed to replace the section of `profile` (a
    /// session switch on the same profile), so it does not ask again. The section that the stop
    /// put back is saved again as the backup.
    async fn activate(&self, id: SessionId, profile: &str, consent_carried: bool) -> Result<()> {
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
                    let key = backup_key(profile);
                    // A backup under this key that no managed profile owns may be the only copy
                    // of an earlier user section. Overwrite it only if it equals the section in
                    // the file now (a stale copy: no data can be lost). Else the user decides.
                    if let Some(saved) = secrets.get(&key)?
                        && file.section_text(profile)?.as_deref() != Some(saved.as_str())
                    {
                        return Err(Error::BackupConflict(profile.to_string()));
                    }
                    if !consent_carried
                        && !self
                            .config
                            .prompter
                            .confirm_profile_overwrite(profile)
                            .await
                    {
                        return Err(Error::ProfileOverwriteDeclined(profile.to_string()));
                    }
                    if let Some(text) = file.section_text(profile)? {
                        secrets.set(&key, &text)?;
                        new_backup = Some(key);
                        had_original = true;
                    }
                }
                _ => return Err(Error::DuplicateProfileSection(profile.to_string())),
            }
        }

        match self
            .write_active(id, profile, &session, &file, had_original, managed_already)
            .await
        {
            Ok(()) => Ok(()),
            Err((e, fate)) => {
                // The backup is the only copy of the user's section. Delete it only if the
                // credentials file and the workspace are back to how they were before.
                if fate == BackupFate::Delete
                    && let Some(key) = new_backup
                {
                    let _ = secrets.delete(&key);
                }
                Err(e)
            }
        }
    }

    /// Write-ahead order: the managed entry is saved before the file is written, so a crash
    /// after the file write still lets `startup` put the user's section back from the backup.
    /// On error, the second value says if the caller may delete the backup.
    async fn write_active(
        &self,
        id: SessionId,
        profile: &str,
        session: &crate::model::Session,
        file: &CredentialsFile,
        had_original: bool,
        managed_already: bool,
    ) -> std::result::Result<(), (Error, BackupFate)> {
        let (body, expiration) = match session.kind {
            SessionKind::IamUser { .. } => self
                .iam_user_credentials(session)
                .await
                .map_err(|e| (e, BackupFate::Delete))?,
        };

        let mut ws = self.workspace.lock().unwrap();
        // `shutdown` cleaned up while we waited. Nothing is written yet, so the new backup (if
        // any) is a copy of the section that is still in the file and may go.
        if self.closing.load(Ordering::SeqCst) {
            return Err((Error::Closing, BackupFate::Delete));
        }
        let previous_section = file
            .section_text(profile)
            .map_err(|e| (e, BackupFate::Delete))?;

        // Save the entry first. In memory the session stays `Pending`; on disk it is `Inactive`,
        // so that a crash leaves no `Pending` status.
        let mut ahead = ws.clone();
        ahead.managed_profiles.insert(
            profile.to_string(),
            ManagedProfile {
                session_id: id,
                had_original,
            },
        );
        store::save(&self.workspace_path, &ahead).map_err(|e| (e, BackupFate::Delete))?;
        *ws = ahead;

        if let Err(e) = file.write_section(profile, &body) {
            let fate = self.drop_entry(&mut ws, profile, managed_already);
            return Err((e, fate));
        }

        let mut next = ws.clone();
        if let Some(s) = next.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Active;
            s.start_time = Some(self.config.clock.now());
            s.session_token_expiration = Some(expiration);
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            let fate = match restore(file, profile, previous_section.as_deref()) {
                Ok(()) => self.drop_entry(&mut ws, profile, managed_already),
                // The file still holds our section. Keep the backup and the entry, so that
                // `startup` can try again. The error of the save is returned.
                Err(_) => BackupFate::Keep,
            };
            return Err((e, fate));
        }
        *ws = next;
        Ok(())
    }

    /// Takes the write-ahead entry out again after the file is back to how it was. An entry that
    /// existed before this start stays. If the save fails, the entry stays on disk and the backup
    /// must stay too, because `startup` restores from it.
    fn drop_entry(&self, ws: &mut Workspace, profile: &str, managed_already: bool) -> BackupFate {
        if managed_already {
            return BackupFate::Delete;
        }
        let mut next = ws.clone();
        next.managed_profiles.remove(profile);
        match store::save(&self.workspace_path, &next) {
            Ok(()) => {
                *ws = next;
                BackupFate::Delete
            }
            Err(_) => BackupFate::Keep,
        }
    }
}

/// Whether a failed start may delete the backup of the user's section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupFate {
    Delete,
    Keep,
}

/// What `release_profile` changed, so that a failed save can undo it.
struct Released {
    profile: String,
    previous_section: Option<String>,
    backup_key: Option<String>,
    changed: bool,
}

impl Released {
    /// Puts the section back after a failed save. The save error is the one the caller returns,
    /// so an error here is dropped; the backup is kept, because `finish` runs only after a save.
    fn rollback(&self, file: &CredentialsFile) {
        if self.changed {
            let _ = restore(file, &self.profile, self.previous_section.as_deref());
        }
    }

    /// Deletes the backup. The workspace is already saved, so an error here is dropped. The
    /// backup then stays as an orphan. The next start on the profile reuses it if it equals the
    /// user's section, and else returns `Error::BackupConflict`.
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

/// Puts the section back as it was, after a failed save.
fn restore(file: &CredentialsFile, profile: &str, previous_section: Option<&str>) -> Result<()> {
    match previous_section {
        Some(text) => file.write_raw_section(profile, text),
        None => file.remove_section(profile),
    }
}
