//! Edit and delete of sessions (F1-R3, F1-R4, F1-R6, spec 10.3 without the cascade, F1-R9).

use super::{pending_error, pending_on, profile_name};
use crate::error::{Error, Field, Result};
use crate::model::{IamUserUpdate, SessionId, SessionKind, SessionStatus};
use crate::secrets::SecretStore;
use crate::{Leapp, profile_id_for, store, validate};

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

        let (was_active, old_profile, old_region, old_mfa) = {
            let state = self.state.lock().unwrap();
            let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
            if session.status == SessionStatus::Pending {
                return Err(pending_error());
            }
            let SessionKind::IamUser { mfa_device, .. } = &session.kind;
            (
                session.status == SessionStatus::Active,
                profile_name(&state.ws, session.profile_id)?,
                session.region.clone(),
                mfa_device.clone(),
            )
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

        if let Some(change) = &key_change {
            change.write(secrets, &names)?;
        }
        let saved = {
            let mut state = self.state.lock().unwrap();
            let expected = if was_active && !restart {
                SessionStatus::Active
            } else {
                SessionStatus::Inactive
            };
            match state.ws.session(id) {
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
                    let SessionKind::IamUser {
                        mfa_device,
                        account,
                    } = &mut session.kind;
                    *mfa_device = valid.mfa_device.clone();
                    if keys_changed {
                        *account = None;
                    }
                    store::save(&self.workspace_path, &next).map(|()| {
                        state.ws = next;
                        if keys_changed {
                            *state.key_generation.entry(id).or_default() += 1;
                        }
                    })
                }
            }
        };
        if let Err(e) = saved {
            if let Some(change) = &key_change {
                change.undo(secrets, &names);
            }
            return Err(e);
        }
        // The expiration is cleared, so a token that this delete misses is not reused.
        if keys_changed || mfa_changed {
            let _ = secrets.delete(&names.token);
        }

        if restart {
            self.start(id).await?;
        }
        match backup_missing {
            Some(p) => Err(Error::BackupMissing(p)),
            None => Ok(()),
        }
    }

    /// Deletes a session (spec 10.3 without the chained cascade and pins, F1-R9): stops it if it
    /// is `Active`, saves the workspace without it, then deletes its keychain keys. A failed key
    /// delete is dropped (F1-R6). A `Pending` session returns `Error::ProfilePending` and stays.
    /// If the stop finds the user's backup missing, the delete completes and then returns
    /// `Error::BackupMissing` (F1-R8).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn delete_session(&self, id: SessionId) -> Result<()> {
        // `stop` returns `NotFound` and `ProfilePending` itself, and does nothing if `Inactive`.
        let backup_missing = match self.stop(id).await {
            Ok(()) => None,
            Err(Error::BackupMissing(p)) => Some(p),
            Err(e) => return Err(e),
        };
        {
            let mut state = self.state.lock().unwrap();
            match state.ws.session(id) {
                None => return Err(Error::NotFound(id)),
                Some(s) if s.status != SessionStatus::Inactive => return Err(changed_meanwhile()),
                Some(_) => {}
            }
            let mut next = state.ws.clone();
            next.sessions.retain(|s| s.id != id);
            store::save(&self.workspace_path, &next)?;
            state.ws = next;
            state.key_generation.remove(&id);
        }
        let names = KeyNames::of(id);
        let secrets = self.config.secrets.as_ref();
        for key in [names.access_key_id, names.secret_access_key, names.token] {
            let _ = secrets.delete(&key);
        }
        match backup_missing {
            Some(p) => Err(Error::BackupMissing(p)),
            None => Ok(()),
        }
    }
}

/// A start or stop of the same session ran while the edit or delete waited.
fn changed_meanwhile() -> Error {
    Error::ProfilePending("The session started or stopped during the change. Try again.".into())
}
