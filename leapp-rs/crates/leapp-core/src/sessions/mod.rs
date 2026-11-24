//! Start and stop of sessions (spec 10.1, 10.2).

mod edit;
mod iam_user;

use iam_user::Issued;
pub use iam_user::TOKEN_REUSE_MARGIN;

use std::sync::Mutex;

use uuid::Uuid;

use crate::aws_files::{CredentialsFile, SectionBody};
use crate::error::{Error, Field, Result};
use crate::model::{ManagedProfile, SessionId, SessionKind, SessionStatus, Workspace};
use crate::refresh::{Refresh, is_due};
use crate::secrets::SecretStore;
use crate::store;
use crate::{Leapp, State};

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
            let state = self.state.lock().unwrap();
            if state.closing {
                return Err(Error::Closing);
            }
            let ws = &state.ws;
            let session = ws.session(id).ok_or(Error::NotFound(id))?;
            let profile = profile_name(ws, session.profile_id)?;
            if pending_on(ws, session.profile_id) {
                return Err(pending_error());
            }
            if state.editing.contains(&id) {
                return Err(editing_error());
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
        // An edit is refused while the session is `Pending`, and this start is refused while an
        // edit has the session reserved, so the keys cannot change between here and the key
        // read of this start. The check is in the same critical section that sets `Pending`.
        let key_generation = {
            let mut state = self.state.lock().unwrap();
            if state.closing {
                return Err(Error::Closing);
            }
            if pending_on(&state.ws, profile_id) {
                return Err(pending_error());
            }
            if state.editing.contains(&id) {
                return Err(editing_error());
            }
            set_status(&mut state.ws, id, SessionStatus::Pending);
            key_generation(&state, id)
        };
        let pending = PendingGuard::arm(&self.state, id);

        let result = self
            .activate_and_store_account(id, &profile, consent_carried, key_generation)
            .await;
        if result.is_err() {
            let mut state = self.state.lock().unwrap();
            set_status(&mut state.ws, id, SessionStatus::Inactive);
        }
        pending.disarm();
        result
    }

    /// Refreshes one old `Active` session (spec 10.4): steps 3 to 6 of 10.1 on the profile that
    /// the session already manages, so no overwrite question. The session is skipped (no
    /// failure) if it is no longer due, an edit has it reserved, a stop of its profile still
    /// deletes a backup, or `shutdown` began. On an error the profile is released as `stop`
    /// does and the session is `Inactive`.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub(crate) async fn refresh_session(&self, id: SessionId) -> Refresh {
        let (profile, name, key_generation) = {
            let mut state = self.state.lock().unwrap();
            if state.closing {
                return Refresh::Skipped;
            }
            let now = self.config.clock.now();
            let Some(session) = state.ws.session(id).filter(|s| is_due(s, now)) else {
                return Refresh::Skipped;
            };
            let Ok(profile) = profile_name(&state.ws, session.profile_id) else {
                return Refresh::Skipped;
            };
            if state.editing.contains(&id)
                || state.releasing.contains(&profile)
                || pending_on(&state.ws, session.profile_id)
            {
                return Refresh::Skipped;
            }
            let name = session.name.clone();
            set_status(&mut state.ws, id, SessionStatus::Pending);
            (profile, name, key_generation(&state, id))
        };
        // A panic leaves the profile managed and our section in the file; `shutdown`, `startup`
        // or the next start of the profile puts it right. The session must not stay `Pending`.
        let pending = PendingGuard::arm(&self.state, id);

        let refresh = match self
            .activate_and_store_account(id, &profile, false, key_generation)
            .await
        {
            Ok(()) => Refresh::Done,
            Err(e) => {
                // After `shutdown` began, any error is `Closing`: `shutdown` cleans up the
                // profile, so no release here and no failure for the user.
                let closing = matches!(e, Error::Closing) || self.state.lock().unwrap().closing;
                // `shutdown` releases the profile itself.
                let released = if closing {
                    Ok(())
                } else {
                    self.release(id, true)
                };
                {
                    // A failed release leaves the session `Pending`; the managed entry and the
                    // backup stay, so `shutdown` or `startup` puts the user's section back.
                    let mut state = self.state.lock().unwrap();
                    if state.ws.session(id).map(|s| s.status) == Some(SessionStatus::Pending) {
                        set_inactive(&mut state.ws, id);
                    }
                }
                if closing {
                    return Refresh::Skipped;
                }
                let mut errors = vec![e];
                errors.extend(released.err());
                Refresh::Failed { name, errors }
            }
        };
        pending.disarm();
        refresh
    }

    /// `activate`, then the account fetch if it got a new token (F1-R5).
    async fn activate_and_store_account(
        &self,
        id: SessionId,
        profile: &str,
        consent_carried: bool,
        key_generation: u64,
    ) -> Result<()> {
        let fresh = self.activate(id, profile, consent_carried).await?;
        // Only an activation that called `GetSessionToken` gets here with credentials, and only
        // while the session has no account.
        if let Some(temp) = fresh
            && self.account_missing(id)
        {
            self.store_account(id, &temp, key_generation).await;
        }
        Ok(())
    }

    fn account_missing(&self, id: SessionId) -> bool {
        let state = self.state.lock().unwrap();
        state
            .ws
            .session(id)
            .is_some_and(|s| matches!(&s.kind, SessionKind::IamUser { account: None, .. }))
    }

    /// Stops a session: puts its profile section back as it was (the user's saved section, or
    /// none). Does nothing if it is `Inactive`. A `Pending` session cannot be stopped.
    ///
    /// The keychain read of the backup runs without the workspace lock. If the managed entry
    /// changed meanwhile, the stop starts again from the top one time; after a second change it
    /// reads the backup under the lock. If the profile had a user section but its backup is
    /// missing, the stop removes our section, completes, and returns `Error::BackupMissing`.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block (the
    /// UI calls it from its tokio bridge, not the render thread).
    pub async fn stop(&self, id: SessionId) -> Result<()> {
        self.release(id, false)
    }

    /// The body of `stop`. With `owner_pending`, the caller set the session `Pending` itself (a
    /// failed refresh) and releases it from that status; else a `Pending` session is refused.
    fn release(&self, id: SessionId, owner_pending: bool) -> Result<()> {
        let mut retried = false;
        loop {
            let seen = stop_target(&self.state.lock().unwrap(), id, owner_pending)?;
            let Some((profile, entry)) = seen else {
                return Ok(());
            };
            let backup = self.read_backup(&profile, entry.as_ref(), id)?;

            let mut state = self.state.lock().unwrap();
            let Some((now_profile, now_entry)) = stop_target(&state, id, owner_pending)? else {
                return Ok(());
            };
            let backup = if now_profile == profile && now_entry == entry {
                backup
            } else if !retried {
                retried = true;
                continue;
            } else {
                // Rare: the entry changed two times. Read the backup under the lock this time.
                self.read_backup(&now_profile, now_entry.as_ref(), id)?
            };
            let profile = now_profile;

            let file = CredentialsFile::new(&self.config.home_dir);
            let mut next = state.ws.clone();
            let released = release_profile(&mut next, &file, &profile, id, backup)?;
            set_inactive(&mut next, id);
            if let Err(e) = store::save(&self.workspace_path, &next) {
                released.rollback(&file);
                return Err(e);
            }
            state.ws = next;
            let _mark = Releasing::mark(&self.state, &mut state, &released);
            drop(state);
            released.finish(self.config.secrets.as_ref());
            return if released.backup_missing {
                Err(Error::BackupMissing(profile))
            } else {
                Ok(())
            };
        }
    }

    /// The backup of the user's section of `profile`, if `owner` manages the profile and the
    /// profile had a user section. Call it without the workspace lock: it reads the keychain.
    fn read_backup(
        &self,
        profile: &str,
        entry: Option<&ManagedProfile>,
        owner: SessionId,
    ) -> Result<Option<String>> {
        match entry {
            Some(m) if m.session_id == owner && m.had_original => {
                self.config.secrets.get(&backup_key(profile))
            }
            _ => Ok(None),
        }
    }

    /// Cleans up after a crash: every managed profile is restored or removed, every session is
    /// set to `Inactive`.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block (the
    /// UI calls it from its tokio bridge, not the render thread).
    pub async fn startup(&self) -> Result<()> {
        self.clean_all(false)
    }

    /// Same cleanup as `startup`, for a normal exit. It is final: a start that is still in
    /// flight (for example one that waits on AWS) fails with `Error::Closing` before it writes,
    /// and later starts fail the same way.
    ///
    /// This function does blocking file and keychain I/O. The UI calls it on the render thread, with `block_on` in `on_close`, because the window must
    /// not close before the cleanup ends. That is acceptable: the work is local (the credentials
    /// file, `workspace.json` and the keychain), with no network call. A keychain prompt can
    /// keep the window open until the user answers it.
    pub async fn shutdown(&self) -> Result<()> {
        self.clean_all(true)
    }

    /// With `closing`, sets the closing flag under the lock first, so no start can write after
    /// the cleanup.
    ///
    /// The backups are read without the lock. A managed entry that changed meanwhile is skipped;
    /// the next `startup` cleans it. If a backup is missing, the cleanup completes and then
    /// returns `Error::BackupMissing` for the first such profile.
    fn clean_all(&self, closing: bool) -> Result<()> {
        let seen: Vec<(String, ManagedProfile)> = {
            let mut state = self.state.lock().unwrap();
            if closing {
                state.closing = true;
            }
            state
                .ws
                .managed_profiles
                .iter()
                .map(|(profile, m)| (profile.clone(), m.clone()))
                .collect()
        };
        let mut backups = Vec::with_capacity(seen.len());
        for (profile, entry) in seen {
            let backup = self.read_backup(&profile, Some(&entry), entry.session_id)?;
            backups.push((profile, entry, backup));
        }

        let mut state = self.state.lock().unwrap();
        let file = CredentialsFile::new(&self.config.home_dir);
        let mut next = state.ws.clone();
        let mut released = Vec::new();
        for (profile, entry, backup) in backups {
            if next.managed_profiles.get(&profile) != Some(&entry) {
                continue;
            }
            match release_profile(&mut next, &file, &profile, entry.session_id, backup) {
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
        state.ws = next;
        let _marks: Vec<Releasing<'_>> = released
            .iter()
            .map(|r| Releasing::mark(&self.state, &mut state, r))
            .collect();
        drop(state);
        for r in &released {
            r.finish(self.config.secrets.as_ref());
        }
        match released.into_iter().find(|r| r.backup_missing) {
            Some(r) => Err(Error::BackupMissing(r.profile)),
            None => Ok(()),
        }
    }

    /// Steps 3 to 7 of spec 10.1 and 9.3. Changes the file and the workspace together or not at
    /// all. The backup of the user's section is deleted on failure only when the file and the
    /// workspace are back as before; else it stays, because it is the only copy.
    ///
    /// With `consent_carried`, the user already agreed to replace the section of `profile` (a
    /// session switch on the same profile), so it does not ask again. The section that the stop
    /// put back is saved again as the backup.
    async fn activate(
        &self,
        id: SessionId,
        profile: &str,
        consent_carried: bool,
    ) -> Result<Option<SectionBody>> {
        let (session, existing) = {
            let state = self.state.lock().unwrap();
            // A stop of this profile still deletes the old backup. A new backup now would be
            // deleted with it.
            if state.releasing.contains(profile) {
                return Err(releasing_error(profile));
            }
            (
                state.ws.session(id).ok_or(Error::NotFound(id))?.clone(),
                state.ws.managed_profiles.get(profile).cloned(),
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
                    // After `shutdown` began, no new backup.
                    if self.state.lock().unwrap().closing {
                        return Err(Error::Closing);
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
            Ok(fresh) => Ok(fresh),
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
    ) -> std::result::Result<Option<SectionBody>, (Error, BackupFate)> {
        let Issued {
            body,
            expiration,
            fresh,
        } = match session.kind {
            SessionKind::IamUser { .. } => self
                .iam_user_credentials(session)
                .await
                .map_err(|e| (e, BackupFate::Delete))?,
        };

        let mut state = self.state.lock().unwrap();
        // This is the check that counts: it is in the critical section that writes. The
        // `closing` checks before it only end a start early.
        // `shutdown` cleaned up while we waited. Nothing is written yet, so the new backup (if
        // any) is a copy of the section that is still in the file and may go.
        if state.closing {
            return Err((Error::Closing, BackupFate::Delete));
        }
        let previous_section = file
            .section_text(profile)
            .map_err(|e| (e, BackupFate::Delete))?;

        // Save the entry first. In memory the session stays `Pending`; on disk it is `Inactive`,
        // so that a crash leaves no `Pending` status.
        let mut ahead = state.ws.clone();
        ahead.managed_profiles.insert(
            profile.to_string(),
            ManagedProfile {
                session_id: id,
                had_original,
            },
        );
        store::save(&self.workspace_path, &ahead).map_err(|e| (e, BackupFate::Delete))?;
        state.ws = ahead;

        if let Err(e) = file.write_section(profile, &body) {
            let fate = self.drop_entry(&mut state.ws, profile, managed_already);
            return Err((e, fate));
        }

        let mut next = state.ws.clone();
        if let Some(s) = next.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Active;
            s.start_time = Some(self.config.clock.now());
            s.session_token_expiration = Some(expiration);
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            let fate = match restore(file, profile, previous_section.as_deref()) {
                Ok(()) => self.drop_entry(&mut state.ws, profile, managed_already),
                // The file still holds our section. Keep the backup and the entry, so that
                // `startup` can try again. The error of the save is returned.
                Err(_) => BackupFate::Keep,
            };
            return Err((e, fate));
        }
        state.ws = next;
        Ok(fresh.then_some(body))
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
    /// The profile had a user section, but its backup was not in the keychain.
    backup_missing: bool,
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

/// Marks a profile as releasing while its backup is deleted outside the lock. The drop takes the
/// lock again and removes the mark, on every path. Create it under the lock, in the same critical
/// section that removes the managed entry; drop it only after the guard is dropped.
struct Releasing<'a> {
    state: &'a Mutex<State>,
    profile: Option<String>,
}

impl<'a> Releasing<'a> {
    /// Marks the profile of `released` if it has a backup to delete; else does nothing.
    fn mark(state: &'a Mutex<State>, locked: &mut State, released: &Released) -> Releasing<'a> {
        let profile = released
            .backup_key
            .as_ref()
            .map(|_| released.profile.clone());
        if let Some(p) = &profile {
            locked.releasing.insert(p.clone());
        }
        Releasing { state, profile }
    }
}

impl Drop for Releasing<'_> {
    fn drop(&mut self) {
        if let Some(p) = &self.profile {
            // A poisoned lock still holds valid data here: remove the mark all the same.
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.releasing.remove(p);
        }
    }
}

/// Sets a session back to `Inactive` if the start or refresh that set it `Pending` ends without
/// a normal return: a panic or a dropped future. Without it the session stays `Pending` until
/// a restart, and every start, stop, edit and delete of it is refused. Create it right after the
/// critical section that sets `Pending`; call `disarm` on the normal paths, which set the status
/// themselves. The drop changes only the memory; `startup` sets every status on disk again.
struct PendingGuard<'a> {
    state: &'a Mutex<State>,
    id: SessionId,
    armed: bool,
}

impl<'a> PendingGuard<'a> {
    fn arm(state: &'a Mutex<State>, id: SessionId) -> PendingGuard<'a> {
        PendingGuard {
            state,
            id,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // A poisoned lock still holds valid data here: clear the status all the same.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.ws.session(self.id).map(|s| s.status) == Some(SessionStatus::Pending) {
            set_inactive(&mut state.ws, self.id);
        }
    }
}

/// The profile of an `Active` session (with `owner_pending`: a `Pending` one) and its managed
/// entry. `None` if the session is `Inactive`.
fn stop_target(
    state: &State,
    id: SessionId,
    owner_pending: bool,
) -> Result<Option<(String, Option<ManagedProfile>)>> {
    let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
    match (session.status, owner_pending) {
        (SessionStatus::Inactive, _) => return Ok(None),
        (SessionStatus::Active, false) | (SessionStatus::Pending, true) => {}
        _ => return Err(pending_error()),
    }
    let profile = profile_name(&state.ws, session.profile_id)?;
    let entry = state.ws.managed_profiles.get(&profile).cloned();
    Ok(Some((profile, entry)))
}

/// Spec 9.3 step 6 for one profile: if `owner` manages it, put the user's saved section back
/// (or remove ours) and drop the entry from `next`. `backup` is the saved section that the caller
/// read from the keychain before it took the lock. If the profile had a user section and
/// `backup` is `None`, our section is removed and the result records the missing backup. The
/// secret backup is not deleted here; `Released::finish` does that after the workspace is saved.
fn release_profile(
    next: &mut Workspace,
    file: &CredentialsFile,
    profile: &str,
    owner: SessionId,
    backup: Option<String>,
) -> Result<Released> {
    let mut released = Released {
        profile: profile.to_string(),
        previous_section: None,
        backup_key: None,
        changed: false,
        backup_missing: false,
    };
    let Some(managed) = next.managed_profiles.get(profile) else {
        return Ok(released);
    };
    if managed.session_id != owner {
        return Ok(released);
    }
    let had_original = managed.had_original;
    released.previous_section = file.section_text(profile)?;
    match (had_original, backup) {
        (true, Some(text)) => {
            file.write_raw_section(profile, &text)?;
            released.backup_key = Some(backup_key(profile));
        }
        (true, None) => {
            file.remove_section(profile)?;
            released.backup_missing = true;
        }
        (false, _) => file.remove_section(profile)?,
    }
    released.changed = true;
    next.managed_profiles.remove(profile);
    Ok(released)
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
            field: Field::Profile,
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

fn editing_error() -> Error {
    Error::ProfilePending("The session is being changed. Try again.".into())
}

fn releasing_error(profile: &str) -> Error {
    Error::ProfilePending(format!(
        "Another session with the named profile \"{profile}\" is stopping"
    ))
}

fn key_generation(state: &State, id: SessionId) -> u64 {
    state.key_generation.get(&id).copied().unwrap_or(0)
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
