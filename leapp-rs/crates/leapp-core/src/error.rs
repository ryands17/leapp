use uuid::Uuid;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("session {0} not found")]
    NotFound(Uuid),
    #[error("invalid {field}: {message}")]
    InvalidInput {
        field: &'static str,
        message: String,
    },
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
