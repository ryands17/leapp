//! Named profiles (spec 12): the name rules and the create.

use uuid::Uuid;

use crate::error::{Error, Field, Result};
use crate::model::{DEFAULT_PROFILE, NamedProfile, Workspace};
use crate::{Leapp, check_profile_chars, store};

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
        return Err(invalid("Empty profile name"));
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
