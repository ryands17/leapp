use std::fmt;

use uuid::Uuid;

/// The input field that an `Error::InvalidInput` is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    AccessKeyId,
    SecretAccessKey,
    Profile,
    Region,
    MfaDevice,
    ParentSession,
    RoleArn,
    RoleSessionName,
    ExternalId,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Field::Name => "name",
            Field::AccessKeyId => "access_key_id",
            Field::SecretAccessKey => "secret_access_key",
            Field::Profile => "profile",
            Field::Region => "region",
            Field::MfaDevice => "mfa_device",
            Field::ParentSession => "parent_session",
            Field::RoleArn => "role_arn",
            Field::RoleSessionName => "role_session_name",
            Field::ExternalId => "external_id",
        })
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("session {0} not found")]
    NotFound(Uuid),
    #[error("session {0} has another type")]
    WrongSessionType(Uuid),
    #[error("the parent sessions of \"{0}\" make a loop")]
    ChainLoop(String),
    #[error("named profile {0} not found")]
    ProfileNotFound(Uuid),
    #[error("invalid {field}: {message}")]
    InvalidInput { field: Field, message: String },
    #[error("{0}")]
    Aws(String),
    #[error("overwrite of profile \"{0}\" was declined")]
    ProfileOverwriteDeclined(String),
    #[error("{0}")]
    ProfilePending(String),
    #[error("profile section \"{0}\" is in the credentials file more than one time")]
    DuplicateProfileSection(String),
    #[error(
        "a saved copy of profile \"{0}\" is in the keychain item \"aws-profile-backup-{0}\" \
         (service leapp-rs) and differs from ~/.aws/credentials. Restore it to \
         ~/.aws/credentials or delete the keychain item, then try again."
    )]
    BackupConflict(String),
    #[error(
        "the saved copy of profile \"{0}\" is missing from the keychain item \
         \"aws-profile-backup-{0}\" (service leapp-rs). Leapp RS removed its own keys from the \
         profile, but could not put your keys back."
    )]
    BackupMissing(String),
    #[error("Missing Multi Factor Authentication code")]
    MfaCancelled,
    #[error("Leapp RS is closing, so the session did not start")]
    Closing,
    #[error("workspace file version {0} is newer than this app supports")]
    UnsupportedVersion(u32),
    #[error("secret store error: {0}")]
    Secret(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
