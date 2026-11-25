//! Named profiles (spec 12): the name rules, the create, the rename and the delete.

use uuid::Uuid;

use crate::error::{Error, Field, Result};
use crate::model::{DEFAULT_PROFILE, NamedProfile, SessionId, SessionStatus, Workspace};
use crate::sessions::{pending_error, pending_on};
use crate::{Leapp, check_profile_chars, profile_id_for, store};

impl Leapp {
    /// Adds a named profile. The name is trimmed and checked with `validate_new_profile_name`.
    pub fn create_profile(&self, name: &str) -> Result<Uuid> {
        let mut state = self.state.lock().unwrap();
        let name = validate_new_profile_name(&state.ws, name, None)?;
        let id = Uuid::new_v4();
        let mut next = state.ws.clone();
        next.profiles.push(NamedProfile { id, name });
        store::save(&self.workspace_path, &next)?;
        state.ws = next;
        Ok(id)
    }

    /// Gives the named profile `profile_id` a new name, checked with
    /// `validate_new_profile_name`. The profile keeps its ID. The `default` profile cannot be
    /// renamed (F23-R2). The same name does nothing (F23-R3). The `Active` session on the
    /// profile is stopped, saved and started again on the new name (F23-R4); its start asks
    /// before it replaces a section that the user wrote under the new name.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn rename_profile(&self, profile_id: Uuid, name: &str) -> Result<()> {
        let (name, ids) = {
            let state = self.state.lock().unwrap();
            let current = changeable(&state.ws, profile_id)?;
            let name = validate_new_profile_name(&state.ws, name, Some(profile_id))?;
            if name == current {
                return Ok(());
            }
            (name, sessions_of(&state.ws, profile_id))
        };
        self.change_sessions(&ids, |ws| {
            // The checks again, under the lock of the save: the workspace can have changed
            // since the checks above.
            changeable(ws, profile_id)?;
            let name = validate_new_profile_name(ws, &name, Some(profile_id))?;
            no_other_session_runs_on(ws, profile_id, &ids)?;
            if let Some(p) = ws.profiles.iter_mut().find(|p| p.id == profile_id) {
                p.name = name;
            }
            Ok(())
        })
        .await
    }

    /// Removes the named profile `profile_id`. Its sessions move to the `default` profile. The
    /// `default` profile cannot be deleted (F23-R2). The `Active` session on the profile is
    /// stopped, saved and started again on `default` (F23-R4); that start stops the session
    /// that is `Active` on `default` (one active session per profile).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn delete_profile(&self, profile_id: Uuid) -> Result<()> {
        let ids = {
            let state = self.state.lock().unwrap();
            changeable(&state.ws, profile_id)?;
            let ids = sessions_of(&state.ws, profile_id);
            // The restart would fail on `default` while another session starts on it.
            let default = state.ws.profiles.iter().find(|p| p.name == DEFAULT_PROFILE);
            if !ids.is_empty() && default.is_some_and(|p| pending_on(&state.ws, p.id)) {
                return Err(pending_error());
            }
            ids
        };
        self.change_sessions(&ids, |ws| {
            changeable(ws, profile_id)?;
            no_other_session_runs_on(ws, profile_id, &ids)?;
            // A hand-edited file can lack `default`; spec 12 says it always exists.
            let default_id = profile_id_for(ws, DEFAULT_PROFILE);
            for s in ws
                .sessions
                .iter_mut()
                .filter(|s| s.profile_id == profile_id)
            {
                s.profile_id = default_id;
            }
            ws.profiles.retain(|p| p.id != profile_id);
            Ok(())
        })
        .await
    }
}

impl Workspace {
    /// The number of sessions on the named profile `profile_id`.
    pub fn sessions_on(&self, profile_id: Uuid) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.profile_id == profile_id)
            .count()
    }
}

/// The name of the profile `profile_id`, if a rename or delete may change it: it exists and it
/// is not `default` (F23-R2).
fn changeable(ws: &Workspace, profile_id: Uuid) -> Result<String> {
    let name = ws
        .profile_name(profile_id)
        .ok_or(Error::ProfileNotFound(profile_id))?;
    if name == DEFAULT_PROFILE {
        return Err(Error::InvalidInput {
            field: Field::Profile,
            message: "the default profile cannot be renamed or deleted".into(),
        });
    }
    Ok(name.to_string())
}

/// The IDs of the sessions on the profile `profile_id`.
fn sessions_of(ws: &Workspace, profile_id: Uuid) -> Vec<SessionId> {
    ws.sessions
        .iter()
        .filter(|s| s.profile_id == profile_id)
        .map(|s| s.id)
        .collect()
}

/// Refuses the change if a session that the change did not stop is on the profile and not
/// `Inactive` (it came to the profile and started after the change took its IDs). An `Inactive`
/// newcomer is harmless: it changes with the others.
fn no_other_session_runs_on(ws: &Workspace, profile_id: Uuid, ids: &[SessionId]) -> Result<()> {
    let other_runs = ws.sessions.iter().any(|s| {
        s.profile_id == profile_id && s.status != SessionStatus::Inactive && !ids.contains(&s.id)
    });
    if other_runs {
        return Err(Error::ProfilePending(
            "A session started on the named profile during the change. Try again.".into(),
        ));
    }
    Ok(())
}

/// The Leapp error for an empty profile name (`named-profiles-service.ts:117`).
pub(crate) fn empty_profile_name() -> Error {
    Error::InvalidInput {
        field: Field::Profile,
        message: "Empty profile name".into(),
    }
}

/// The checks of a new or renamed profile name (Leapp `named-profiles-service.ts:117`, and the F0
/// rule for the characters). `except` is the profile that a rename changes: its own name is not
/// a duplicate. Returns the trimmed name.
pub(crate) fn validate_new_profile_name(
    ws: &Workspace,
    name: &str,
    except: Option<Uuid>,
) -> Result<String> {
    let name = name.trim();
    let invalid = |message: &str| Error::InvalidInput {
        field: Field::Profile,
        message: message.to_string(),
    };
    if name.is_empty() {
        return Err(empty_profile_name());
    }
    if name == DEFAULT_PROFILE {
        return Err(invalid("\"default\" is not a valid profile name"));
    }
    check_profile_chars(name)?;
    if ws
        .profiles
        .iter()
        .any(|p| p.name == name && Some(p.id) != except)
    {
        return Err(invalid("Profile already exists"));
    }
    Ok(name.to_string())
}
