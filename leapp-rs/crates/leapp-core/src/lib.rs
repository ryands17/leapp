//! Core logic of Leapp RS.

mod aws_files;
mod config;
mod error;
mod model;
pub mod regions;
mod secrets;
mod sessions;
mod store;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use uuid::Uuid;

pub use config::{Clock, Config, Endpoints, Prompter, SystemClock};
pub use error::{Error, Field, Result};
pub use model::{
    ManagedProfile, NamedProfile, NewIamUser, Session, SessionId, SessionKind, SessionStatus,
    Workspace,
};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};
pub use sessions::TOKEN_REUSE_MARGIN;

pub struct Leapp {
    config: Config,
    workspace_path: PathBuf,
    state: Mutex<State>,
}

/// Everything that changes after `open`, under one lock.
struct State {
    ws: Workspace,
    /// Set by `shutdown`. After it is set, no start calls AWS or writes the credentials file,
    /// the workspace or a backup. It is in the same lock as `ws`, so a start sees it before its
    /// write-ahead save or not at all.
    closing: bool,
    /// Profiles whose stop or cleanup restored the user's section and still deletes the backup
    /// (outside the lock). A start on such a profile returns `Error::ProfilePending`, so that it
    /// cannot write a new backup that this delete then removes.
    releasing: HashSet<String>,
}

impl Leapp {
    pub fn open(config: Config) -> Result<Leapp> {
        let workspace_path = config.data_dir.join("workspace.json");
        let workspace = store::load(&workspace_path)?;
        Ok(Leapp {
            config,
            workspace_path,
            state: Mutex::new(State {
                ws: workspace,
                closing: false,
                releasing: HashSet::new(),
            }),
        })
    }

    /// A copy of the current workspace.
    pub fn workspace(&self) -> Workspace {
        self.state.lock().unwrap().ws.clone()
    }

    pub fn create_iam_user(&self, req: NewIamUser) -> Result<SessionId> {
        let name = required(Field::Name, &req.name)?;
        let access_key_id = required(Field::AccessKeyId, &req.access_key_id)?;
        let secret_access_key = required(Field::SecretAccessKey, &req.secret_access_key)?;
        let profile = required(Field::Profile, &req.profile_name)?;
        if profile.contains(['[', ']', '\n', '\r']) {
            return Err(Error::InvalidInput {
                field: Field::Profile,
                message: "must not contain [, ], or line breaks".into(),
            });
        }
        if !regions::REGIONS.contains(&req.region.as_str()) {
            return Err(Error::InvalidInput {
                field: Field::Region,
                message: format!("unknown region \"{}\"", req.region),
            });
        }

        let mfa_device = req
            .mfa_device
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string);

        let id = Uuid::new_v4();
        let secrets = &self.config.secrets;
        let key_id_key = format!("iam-user-{id}-access-key-id");
        let secret_key = format!("iam-user-{id}-secret-access-key");
        let result = (|| -> Result<()> {
            secrets.set(&key_id_key, access_key_id)?;
            secrets.set(&secret_key, secret_access_key)?;

            let mut state = self.state.lock().unwrap();
            let mut next = state.ws.clone();
            let profile_id = match next.profiles.iter().find(|p| p.name == profile) {
                Some(p) => p.id,
                None => {
                    let p = NamedProfile {
                        id: Uuid::new_v4(),
                        name: profile.to_string(),
                    };
                    let pid = p.id;
                    next.profiles.push(p);
                    pid
                }
            };
            next.sessions.push(Session {
                id,
                name: name.to_string(),
                region: req.region.clone(),
                profile_id,
                status: SessionStatus::Inactive,
                start_time: None,
                session_token_expiration: None,
                kind: SessionKind::IamUser {
                    mfa_device,
                    account: None,
                },
            });
            store::save(&self.workspace_path, &next)?;
            state.ws = next;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = secrets.delete(&key_id_key);
            let _ = secrets.delete(&secret_key);
            return Err(e);
        }
        Ok(id)
    }
}

fn required(field: Field, value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::InvalidInput {
            field,
            message: "must not be empty".into(),
        });
    }
    Ok(value)
}
