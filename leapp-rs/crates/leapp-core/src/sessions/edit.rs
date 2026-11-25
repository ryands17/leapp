//! Edit and delete of sessions (F1-R3, F1-R4, F1-R6, spec 10.3 with the cascade of F23-R6, F1-R9).

use std::sync::Mutex;

use super::{editing_error, pending_error, pending_on, profile_name, releasing_error};
use crate::error::{Error, Field, Result};
use crate::model::{IamUserUpdate, SessionId, SessionKind, SessionStatus};
use crate::secrets::SecretStore;
use crate::{Leapp, State, profile_id_for, store, validate};

/// The keychain keys of an IAM User session.
struct KeyNames {
    access_key_id: String,
    secret_access_key: String,
    token: String,
}

impl KeyNames {
    fn of(id: SessionId) -> KeyNames {
        KeyNames {
            access_key_id: format!("iam-user-{id}-access-key-id"),
            secret_access_key: format!("iam-user-{id}-secret-access-key"),
            token: format!("iam-user-{id}-session-token"),
        }
    }
}

/// New long-lived keys (`None`: keep the old one) and the old ones, so that a failed edit can
/// put the old ones back.
struct KeyChange {
    new: [Option<String>; 2],
    old: [Option<String>; 2],
}

impl KeyChange {
    fn names(keys: &KeyNames) -> [&str; 2] {
        [&keys.access_key_id, &keys.secret_access_key]
    }

    fn write(&self, secrets: &dyn SecretStore, keys: &KeyNames) -> Result<()> {
        for (name, value) in Self::names(keys).into_iter().zip(&self.new) {
            let Some(value) = value else { continue };
            if let Err(e) = secrets.set(name, value) {
                self.undo(secrets, keys);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Puts the old keys back. The caller returns the first error, so errors here are dropped.
    fn undo(&self, secrets: &dyn SecretStore, keys: &KeyNames) {
        let touched = Self::names(keys).into_iter().zip(&self.new).zip(&self.old);
        for ((name, _), value) in touched.filter(|((_, new), _)| new.is_some()) {
            let _ = match value {
                Some(v) => secrets.set(name, v),
                None => secrets.delete(name),
            };
        }
    }
}

/// A key field of an edit: `None` or empty after trim means "keep the old key".
fn given(key: &Option<String>) -> Option<&str> {
    key.as_deref().filter(|v| !v.trim().is_empty())
}

impl Leapp {
    /// Edits an IAM User session with the validation of `create_iam_user`. A key field that is
    /// `None` or empty keeps the old key.
    ///
    /// F1-R3: if the session is `Active` and the profile, region, keys or MFA device change, the
    /// session is stopped, saved and started again. A name-only edit does not restart. An edit
    /// of a `Pending` session returns `Error::ProfilePending` and changes nothing.
    /// F1-R4: a change of the keys or of the MFA device deletes the cached token and clears
    /// `session_token_expiration`; a change of the keys also clears `account`.
    ///
    /// If the restart fails, the edit stays saved and the session is `Inactive`; the error of
    /// the start is returned. If the stop finds the user's backup missing, the edit completes
    /// and then returns `Error::BackupMissing` (F1-R8).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn update_iam_user(&self, id: SessionId, req: IamUserUpdate) -> Result<()> {
        let valid = validate(
            &req.name,
            [
                (Field::AccessKeyId, given(&req.access_key_id)),
                (Field::SecretAccessKey, given(&req.secret_access_key)),
            ],
            &req.profile_name,
            &req.region,
            req.mfa_device.as_deref(),
        )?;

        let ((was_active, old_profile, old_region, old_mfa), reservation) = {
            let mut state = self.state.lock().unwrap();
            let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
            if session.status == SessionStatus::Pending {
                return Err(pending_error());
            }
            let SessionKind::IamUser { mfa_device, .. } = &session.kind else {
                return Err(Error::WrongSessionType(id));
            };
            let seen = (
                session.status == SessionStatus::Active,
                profile_name(&state.ws, session.profile_id)?,
                session.region.clone(),
                mfa_device.clone(),
            );
            let reservation = Editing::reserve(&self.state, &mut state, id)?;
            (seen, reservation)
        };

        // The keychain is read without the lock.
        let secrets = self.config.secrets.as_ref();
        let names = KeyNames::of(id);
        let key_change = if valid.keys.iter().any(Option::is_some) {
            let old = [
                secrets.get(&names.access_key_id)?,
                secrets.get(&names.secret_access_key)?,
            ];
            let new = valid.keys.map(|k| k.map(str::to_string));
            let changed = new.iter().zip(&old).any(|(n, o)| n.is_some() && n != o);
            changed.then_some(KeyChange { new, old })
        } else {
            None
        };
        let keys_changed = key_change.is_some();
        let mfa_changed = valid.mfa_device != old_mfa;
        let restart = was_active
            && (keys_changed
                || mfa_changed
                || valid.profile != old_profile
                || req.region != old_region);

        let mut backup_missing = None;
        if restart {
            {
                let state = self.state.lock().unwrap();
                let new_profile = state.ws.profiles.iter().find(|p| p.name == valid.profile);
                if new_profile.is_some_and(|p| pending_on(&state.ws, p.id)) {
                    return Err(pending_error());
                }
            }
            match self.stop(id).await {
                Ok(()) => {}
                Err(Error::BackupMissing(p)) => backup_missing = Some(p),
                Err(e) => return Err(e),
            }
        }

        let written = match &key_change {
            // On a failure, `write` puts the old keys back before it returns.
            Some(change) => change.write(secrets, &names),
            None => Ok(()),
        };
        let saved = written.and_then(|()| {
            let mut state = self.state.lock().unwrap();
            let expected = if was_active && !restart {
                SessionStatus::Active
            } else {
                SessionStatus::Inactive
            };
            let saved = match state.ws.session(id) {
                None => Err(Error::NotFound(id)),
                Some(s) if s.status != expected => Err(changed_meanwhile()),
                Some(_) => {
                    let mut next = state.ws.clone();
                    let profile_id = profile_id_for(&mut next, valid.profile);
                    let session = next
                        .sessions
                        .iter_mut()
                        .find(|s| s.id == id)
                        .expect("the session was found above");
                    session.name = valid.name.to_string();
                    session.region = req.region.clone();
                    session.profile_id = profile_id;
                    if keys_changed || mfa_changed {
                        session.session_token_expiration = None;
                    }
                    // The check above refused other types, and a session never changes its type.
                    if let SessionKind::IamUser {
                        mfa_device,
                        account,
                    } = &mut session.kind
                    {
                        *mfa_device = valid.mfa_device.clone();
                        if keys_changed {
                            *account = None;
                        }
                    }
                    store::save(&self.workspace_path, &next).map(|()| state.ws = next)
                }
            };
            // The keychain is written without the lock.
            drop(state);
            if saved.is_err()
                && let Some(change) = &key_change
            {
                change.undo(secrets, &names);
            }
            saved
        });
        // The keychain and the workspace now hold their final keys and MFA device: the new ones,
        // or the old ones after a failed write or save. The key generation goes up here, once, on
        // every path. A chained start that began before the edit read the generation before it,
        // and it can have read old keys, new keys, rejected keys or a mix: its token is not kept
        // (F23-R7, F1-R4). A chained start that begins during the edit is refused, because the
        // edit's reservation holds the parent. A bump that is not necessary only makes a start
        // get a new token.
        if keys_changed || mfa_changed {
            *self
                .state
                .lock()
                .unwrap()
                .key_generation
                .entry(id)
                .or_default() += 1;
        }
        saved?;
        // The expiration is cleared, so a token that this delete misses is not reused.
        if keys_changed || mfa_changed {
            let _ = secrets.delete(&names.token);
        }

        // The edit is saved or undone; the restart below is a normal start.
        drop(reservation);
        if restart {
            self.start(id).await?;
        }
        match backup_missing {
            Some(p) => Err(Error::BackupMissing(p)),
            None => Ok(()),
        }
    }

    /// Deletes a session and every chained session below it, at any depth (spec 10.3, F23-R6,
    /// without pins), then deletes the keychain keys of the IAM User sessions among them. A
    /// failed key delete is dropped (F1-R6).
    ///
    /// 1. Under the lock: if the session or a session below it is `Pending`, held by an edit or
    ///    a change, or `Active` on a profile that a stop still releases, returns
    ///    `Error::ProfilePending` and changes nothing. Else reserves all of them (all or none),
    ///    so that no start, refresh or edit of them runs until the end.
    /// 2. Stops each `Active` one, the lowest sessions first and the session `id` last, so that
    ///    the user's sections that were backed up go back into the credentials file.
    /// 3. Under the lock: removes all of them in one workspace save. A chained session that came
    ///    below them meanwhile (a create or an edit) is removed too, unless another edit holds it.
    /// 4. Deletes the keychain keys, without the lock.
    ///
    /// If a stop fails (not a missing backup) or step 3 fails, nothing is removed and the error
    /// is returned; the sessions that step 2 stopped before the failure stay `Inactive`. If a
    /// stop finds the user's backup missing, the delete completes and then returns
    /// `Error::BackupMissing` (F1-R8).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn delete_session(&self, id: SessionId) -> Result<()> {
        let (ids, active, reservation) = {
            let mut state = self.state.lock().unwrap();
            state.ws.session(id).ok_or(Error::NotFound(id))?;
            let mut ids = vec![id];
            ids.extend(state.ws.descendants(id));
            let mut active = Vec::new();
            for &i in &ids {
                let s = state
                    .ws
                    .session(i)
                    .expect("`descendants` gives sessions of `ws`");
                match s.status {
                    SessionStatus::Pending => return Err(pending_error()),
                    SessionStatus::Active => {
                        let profile = profile_name(&state.ws, s.profile_id)?;
                        if state.releasing.contains(&profile) {
                            return Err(releasing_error(&profile));
                        }
                        active.push(i);
                    }
                    SessionStatus::Inactive => {}
                }
            }
            // An edit that holds one of them may put keys back under its UUID after the
            // delete, or save a session whose parent is gone.
            let reservation = Editing::reserve_all(&self.state, &mut state, &ids)?;
            (ids, active, reservation)
        };

        // `descendants` is breadth-first, so the reverse order stops the lowest sessions first.
        let mut backup_missing = None;
        for &i in active.iter().rev() {
            match self.stop(i).await {
                Ok(()) => {}
                Err(Error::BackupMissing(p)) => {
                    backup_missing.get_or_insert(p);
                }
                Err(e) => return Err(e),
            }
        }

        let users = {
            let mut state = self.state.lock().unwrap();
            if state.ws.session(id).is_none() {
                return Err(Error::NotFound(id));
            }
            let mut removed = vec![id];
            removed.extend(state.ws.descendants(id));
            let mut users = Vec::new();
            for &i in &removed {
                let s = state
                    .ws
                    .session(i)
                    .expect("`descendants` gives sessions of `ws`");
                if s.status != SessionStatus::Inactive {
                    return Err(changed_meanwhile());
                }
                if !ids.contains(&i) && state.editing.contains(&i) {
                    return Err(editing_error());
                }
                if matches!(s.kind, SessionKind::IamUser { .. }) {
                    users.push(i);
                }
            }
            let mut next = state.ws.clone();
            next.sessions.retain(|s| !removed.contains(&s.id));
            store::save(&self.workspace_path, &next)?;
            state.ws = next;
            for i in &removed {
                state.key_generation.remove(i);
            }
            users
        };
        let secrets = self.config.secrets.as_ref();
        for user in users {
            let names = KeyNames::of(user);
            for key in [names.access_key_id, names.secret_access_key, names.token] {
                let _ = secrets.delete(&key);
            }
        }
        drop(reservation);
        match backup_missing {
            Some(p) => Err(Error::BackupMissing(p)),
            None => Ok(()),
        }
    }
}

/// Reserves sessions for an edit or a change: a start of any of them returns
/// `Error::ProfilePending` until the drop. The drop takes the lock again and removes the marks, on
/// every path. Create it under the lock, in the same critical section that checks that the
/// sessions are not `Pending`; drop it only after the guard is dropped.
pub(super) struct Editing<'a> {
    state: &'a Mutex<State>,
    ids: Vec<SessionId>,
}

impl<'a> Editing<'a> {
    /// Marks `id` as reserved. Returns `Error::ProfilePending` if another edit has it.
    pub(super) fn reserve(
        state: &'a Mutex<State>,
        locked: &mut State,
        id: SessionId,
    ) -> Result<Editing<'a>> {
        Self::reserve_all(state, locked, &[id])
    }

    /// Marks all `ids` as reserved, or none: returns `Error::ProfilePending` if another edit or
    /// change has one of them. `ids` must hold each id one time.
    pub(super) fn reserve_all(
        state: &'a Mutex<State>,
        locked: &mut State,
        ids: &[SessionId],
    ) -> Result<Editing<'a>> {
        if ids.iter().any(|id| locked.editing.contains(id)) {
            return Err(editing_error());
        }
        locked.editing.extend(ids.iter().copied());
        Ok(Editing {
            state,
            ids: ids.to_vec(),
        })
    }
}

impl Drop for Editing<'_> {
    fn drop(&mut self) {
        // A poisoned lock still holds valid data here: remove the marks all the same.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for id in &self.ids {
            state.editing.remove(id);
        }
    }
}

/// A start or stop of the same session ran while the edit or delete waited.
pub(super) fn changed_meanwhile() -> Error {
    Error::ProfilePending("The session started or stopped during the change. Try again.".into())
}
