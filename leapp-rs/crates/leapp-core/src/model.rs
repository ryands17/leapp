use std::collections::BTreeMap;

use jiff::Timestamp;
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
    /// Missing in files from F0 and F1: the defaults.
    #[serde(default)]
    pub settings: Settings,
}

/// The app settings that are saved in `workspace.json`.
/// A missing field gets its default (T1-m5).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// The region that the create forms show first.
    pub default_region: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_region: crate::regions::DEFAULT_REGION.to_string(),
        }
    }
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
            settings: Settings::default(),
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
    pub start_time: Option<Timestamp>,
    pub session_token_expiration: Option<Timestamp>,
    pub kind: SessionKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKind {
    IamUser {
        mfa_device: Option<String>,
        account: Option<String>,
    },
    /// Assumes `role_arn` with the temporary keys of the parent session (spec 12).
    IamRoleChained {
        role_arn: String,
        parent_session_id: SessionId,
        /// `None`: `DEFAULT_ROLE_SESSION_NAME` is sent.
        role_session_name: Option<String>,
        external_id: Option<String>,
    },
}

impl Session {
    /// The AWS account number: the one fetched for an IAM User, the one in the role ARN of a
    /// chained session (F23-R10).
    pub fn account(&self) -> Option<String> {
        match &self.kind {
            SessionKind::IamUser { account, .. } => account.clone(),
            SessionKind::IamRoleChained { role_arn, .. } => {
                role_arn_account(role_arn).map(str::to_string)
            }
        }
    }
}

/// The account number of a role ARN, if it looks like
/// `arn:<aws|aws-cn|aws-us-gov>:iam::<12 digits>:role/<path and name>` (F23-R10).
pub(crate) fn role_arn_account(arn: &str) -> Option<&str> {
    let mut parts = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let partition_ok = matches!(partition, "aws" | "aws-cn" | "aws-us-gov");
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    // The role name is the part after the last `/` of the path; it must not be empty.
    let name_ok = resource.strip_prefix("role/").is_some_and(|path| {
        path.rsplit('/').next().is_some_and(|name| !name.is_empty())
            && !path.chars().any(|c| c.is_whitespace() || c.is_control())
    });
    (partition_ok && account_ok && name_ok).then_some(account)
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

/// A new IAM Role Chained session. It holds no secrets: a role ARN, a role session name and an
/// external ID are not secret.
#[derive(Clone, Debug)]
pub struct NewIamRoleChained {
    pub name: String,
    pub parent_session_id: SessionId,
    pub role_arn: String,
    /// Empty after trim means not set: `DEFAULT_ROLE_SESSION_NAME` is sent.
    pub role_session_name: Option<String>,
    /// Empty after trim means not set.
    pub external_id: Option<String>,
    pub profile_name: String,
    pub region: String,
}

/// New values for an IAM User session. A key field that is `None` or empty after trim keeps the
/// old key.
#[derive(Clone)]
pub struct IamUserUpdate {
    pub name: String,
    pub profile_name: String,
    pub region: String,
    /// Serial number or ARN of the MFA device. Empty after trim means no MFA.
    pub mfa_device: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
}

impl std::fmt::Debug for IamUserUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("IamUserUpdate")
            .field("name", &self.name)
            .field("profile_name", &self.profile_name)
            .field("region", &self.region)
            .field("mfa_device", &self.mfa_device)
            .field("access_key_id", &redacted(&self.access_key_id))
            .field("secret_access_key", &redacted(&self.secret_access_key))
            .finish()
    }
}
