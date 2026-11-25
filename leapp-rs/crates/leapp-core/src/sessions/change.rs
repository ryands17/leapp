//! Change of the region or the named profile of a session (spec 9.3 step 8, 14.7, F23-R1,
//! F23-R3, F23-R4).

use super::edit::{Editing, changed_meanwhile};
use super::{pending_error, pending_on, profile_name};
use crate::error::{Error, Field, Result};
use crate::model::{SessionId, SessionStatus, Workspace};
use crate::{Leapp, check_profile_chars, check_region, profile_id_for, required, store};

impl Leapp {
    /// Moves a session to another region. The same region does nothing (F23-R3). An `Active`
    /// session is stopped, saved and started again (F23-R4).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn change_region(&self, id: SessionId, region: &str) -> Result<()> {
        check_region(region)?;
        {
            let state = self.state.lock().unwrap();
            let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
            if session.region == region {
                return Ok(());
            }
        }
        self.change_sessions(&[id], |ws| {
            session_mut(ws, id)?.region = region.to_string();
            Ok(())
        })
        .await
    }

    /// Moves a session to the named profile `profile` (F23-R1): the name is trimmed, and a
    /// name that no profile has creates the profile. `default` is allowed. The same profile does
    /// nothing (F23-R3). An `Active` session is stopped, saved and started again (F23-R4).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn change_profile(&self, id: SessionId, profile: &str) -> Result<()> {
        let name = required(Field::Profile, profile)?;
        check_profile_chars(name)?;
        {
            let state = self.state.lock().unwrap();
            let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
            if profile_name(&state.ws, session.profile_id)? == name {
                return Ok(());
            }
            // The restart would fail on a profile that another session starts on now.
            let target = state.ws.profiles.iter().find(|p| p.name == name);
            if target.is_some_and(|p| pending_on(&state.ws, p.id)) {
                return Err(pending_error());
            }
        }
        self.change_sessions(&[id], |ws| {
            let profile_id = profile_id_for(ws, name);
            session_mut(ws, id)?.profile_id = profile_id;
            Ok(())
        })
        .await
    }

    /// Applies `change` to the workspace while the sessions `ids` are stopped (F23-R4).
    ///
    /// 1. Under the lock: refuses with `Error::ProfilePending` if a session is `Pending`, held by
    ///    an edit or change, or on a profile that is `Pending`. Reserves all of them, so that no
    ///    start or refresh of them runs until step 4, and notes the `Active` ones.
    /// 2. Stops each `Active` one. A missing backup is kept for the end and the change goes on
    ///    (F1-R8).
    /// 3. Under the lock: checks that each session is still there and `Inactive`, applies
    ///    `change` to a copy of the workspace, saves it.
    /// 4. Drops the reservation and starts again the ones that were `Active`. Returns the first
    ///    start error, else the missing backup, else `Ok`. A failed start leaves the change saved
    ///    and the session `Inactive`.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub(crate) async fn change_sessions(
        &self,
        ids: &[SessionId],
        change: impl FnOnce(&mut Workspace) -> Result<()>,
    ) -> Result<()> {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();

        let (was_active, reservation) = {
            let mut state = self.state.lock().unwrap();
            let mut was_active = Vec::new();
            for &id in &ids {
                let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
                if session.status == SessionStatus::Pending
                    || pending_on(&state.ws, session.profile_id)
                {
                    return Err(pending_error());
                }
                if session.status == SessionStatus::Active {
                    was_active.push(id);
                }
            }
            let reservation = Editing::reserve_all(&self.state, &mut state, &ids)?;
            (was_active, reservation)
        };

        let mut backup_missing = None;
        for &id in &was_active {
            match self.stop(id).await {
                Ok(()) => {}
                Err(Error::BackupMissing(p)) => {
                    backup_missing.get_or_insert(p);
                }
                Err(e) => return Err(e),
            }
        }

        {
            let mut state = self.state.lock().unwrap();
            for &id in &ids {
                match state.ws.session(id) {
                    None => return Err(Error::NotFound(id)),
                    Some(s) if s.status != SessionStatus::Inactive => {
                        return Err(changed_meanwhile());
                    }
                    Some(_) => {}
                }
            }
            let mut next = state.ws.clone();
            change(&mut next)?;
            store::save(&self.workspace_path, &next)?;
            state.ws = next;
        }

        // The change is saved; the restarts below are normal starts.
        drop(reservation);
        let mut first_error = None;
        for &id in &was_active {
            if let Err(e) = self.start(id).await {
                first_error.get_or_insert(e);
            }
        }
        match (first_error, backup_missing) {
            (Some(e), _) => Err(e),
            (None, Some(p)) => Err(Error::BackupMissing(p)),
            (None, None) => Ok(()),
        }
    }
}

/// The session `id` of `ws`, to change it.
fn session_mut(ws: &mut Workspace, id: SessionId) -> Result<&mut crate::model::Session> {
    ws.sessions
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(Error::NotFound(id))
}
