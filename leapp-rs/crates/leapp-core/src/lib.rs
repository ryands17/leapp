//! Core logic of Leapp RS.

mod config;
mod error;
mod model;
pub mod regions;
mod secrets;
mod store;

use std::path::PathBuf;
use std::sync::Mutex;

use uuid::Uuid;

pub use config::{Clock, Config, Endpoints, Prompter, SystemClock};
pub use error::{Error, Result};
pub use model::{
    ManagedProfile, NamedProfile, NewIamUser, Session, SessionId, SessionKind, SessionStatus,
    Workspace,
};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};

pub struct Leapp {
    config: Config,
    workspace_path: PathBuf,
    workspace: Mutex<Workspace>,
}

impl Leapp {
    pub fn open(config: Config) -> Result<Leapp> {
        let workspace_path = config.data_dir.join("workspace.json");
        let workspace = store::load(&workspace_path)?;
        Ok(Leapp {
            config,
            workspace_path,
            workspace: Mutex::new(workspace),
        })
    }

    /// A copy of the current workspace.
    pub fn workspace(&self) -> Workspace {
        self.workspace.lock().unwrap().clone()
    }

    pub fn create_iam_user(&self, req: NewIamUser) -> Result<SessionId> {
        let name = required("name", &req.name)?;
        let access_key_id = required("access_key_id", &req.access_key_id)?;
        let secret_access_key = required("secret_access_key", &req.secret_access_key)?;
        let profile = required("profile", &req.profile_name)?;
        if profile.contains(['[', ']', '\n', '\r']) {
            return Err(Error::InvalidInput {
                field: "profile",
                message: "must not contain [, ], or line breaks".into(),
            });
        }
        if !regions::REGIONS.contains(&req.region.as_str()) {
            return Err(Error::InvalidInput {
                field: "region",
                message: format!("unknown region \"{}\"", req.region),
            });
        }

        let id = Uuid::new_v4();
        let secrets = &self.config.secrets;
        secrets.set(&format!("iam-user-{id}-access-key-id"), access_key_id)?;
        secrets.set(
            &format!("iam-user-{id}-secret-access-key"),
            secret_access_key,
        )?;

        let mut ws = self.workspace.lock().unwrap();
        let profile_id = match ws.profiles.iter().find(|p| p.name == profile) {
            Some(p) => p.id,
            None => {
                let p = NamedProfile {
                    id: Uuid::new_v4(),
                    name: profile.to_string(),
                };
                let pid = p.id;
                ws.profiles.push(p);
                pid
            }
        };
        ws.sessions.push(Session {
            id,
            name: name.to_string(),
            region: req.region,
            profile_id,
            status: SessionStatus::Inactive,
            start_time: None,
            session_token_expiration: None,
            kind: SessionKind::IamUser {
                mfa_device: None,
                account: None,
            },
        });
        store::save(&self.workspace_path, &ws)?;
        Ok(id)
    }
}

fn required<'a>(field: &'static str, value: &'a str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::InvalidInput {
            field,
            message: "must not be empty".into(),
        });
    }
    Ok(value)
}
