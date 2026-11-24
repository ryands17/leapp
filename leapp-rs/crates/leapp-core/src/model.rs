use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type SessionId = Uuid;

pub const WORKSPACE_VERSION: u32 = 1;
pub const DEFAULT_PROFILE: &str = "default";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub version: u32,
    pub sessions: Vec<Session>,
    pub profiles: Vec<NamedProfile>,
    #[serde(default)]
    pub managed_profiles: BTreeMap<String, ManagedProfile>,
}

impl Workspace {
    /// A new workspace with the `default` profile.
    pub fn new() -> Workspace {
        Workspace {
            version: WORKSPACE_VERSION,
            sessions: Vec::new(),
            profiles: vec![NamedProfile {
                id: Uuid::new_v4(),
                name: DEFAULT_PROFILE.to_string(),
            }],
            managed_profiles: BTreeMap::new(),
        }
    }

    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn profile_name(&self, profile_id: Uuid) -> Option<&str> {
        self.profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.name.as_str())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub region: String,
    pub profile_id: Uuid,
    pub status: SessionStatus,
    pub start_time: Option<DateTime<Utc>>,
    pub session_token_expiration: Option<DateTime<Utc>>,
    pub kind: SessionKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKind {
    IamUser {
        mfa_device: Option<String>,
        account: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Inactive,
    Pending,
    Active,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NamedProfile {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManagedProfile {
    pub session_id: SessionId,
    pub had_original: bool,
}

#[derive(Clone)]
pub struct NewIamUser {
    pub name: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub profile_name: String,
    pub region: String,
    /// Serial number or ARN of the MFA device. Empty after trim means no MFA.
    pub mfa_device: Option<String>,
}

impl std::fmt::Debug for NewIamUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewIamUser")
            .field("name", &self.name)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("profile_name", &self.profile_name)
            .field("region", &self.region)
            .field("mfa_device", &self.mfa_device)
            .finish()
    }
}
