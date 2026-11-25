//! Core logic of Leapp RS.

mod aws_files;
mod config;
mod error;
mod model;
mod profiles;
mod refresh;
pub mod regions;
mod secrets;
mod sessions;
mod store;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use uuid::Uuid;

pub use config::{Clock, Config, Endpoints, Prompter, SystemClock};
pub use error::{Error, Field, Result};
pub use model::{
    IamUserUpdate, ManagedProfile, NamedProfile, NewIamUser, Session, SessionId, SessionKind,
    SessionStatus, Settings, Workspace,
};
pub use refresh::{REFRESH_TICK, RefreshFailure, RefreshReport, SESSION_DURATION};
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
    /// Bumped by each edit that changes the keys of a session (missing = 0). A start notes it
    /// when it sets `Pending`; its account fetch saves only if it is unchanged, so an account
    /// fetched with the old keys never lands after the edit cleared it (F1-R4, F1-R5).
    key_generation: HashMap<SessionId, u64>,
    /// Sessions that an edit changes now. The edit writes the keys outside the lock before it
    /// saves; a start of such a session returns `Error::ProfilePending`, so that it cannot read
    /// keys that the edit may still put back.
    editing: HashSet<SessionId>,
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
                key_generation: HashMap::new(),
                editing: HashSet::new(),
            }),
        })
    }

    /// A copy of the current workspace.
    pub fn workspace(&self) -> Workspace {
        self.state.lock().unwrap().ws.clone()
    }

    /// Saves the settings. The default region must be one of `regions::REGIONS`.
    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        check_region(&settings.default_region)?;
        let mut state = self.state.lock().unwrap();
        let mut next = state.ws.clone();
        next.settings = settings;
        store::save(&self.workspace_path, &next)?;
        state.ws = next;
        Ok(())
    }

    pub fn create_iam_user(&self, req: NewIamUser) -> Result<SessionId> {
        let valid = validate(
            &req.name,
            [
                (Field::AccessKeyId, Some(&req.access_key_id)),
                (Field::SecretAccessKey, Some(&req.secret_access_key)),
            ],
            &req.profile_name,
            &req.region,
            req.mfa_device.as_deref(),
        )?;
        let Valid {
            name,
            keys,
            profile,
            mfa_device,
        } = valid;
        // Create gives both keys, so `validate` returns both as `Some`.
        let [access_key_id, secret_access_key] = keys.map(Option::unwrap_or_default);

        let id = Uuid::new_v4();
        let secrets = &self.config.secrets;
        let key_id_key = format!("iam-user-{id}-access-key-id");
        let secret_key = format!("iam-user-{id}-secret-access-key");
        let result = (|| -> Result<()> {
            secrets.set(&key_id_key, access_key_id)?;
            secrets.set(&secret_key, secret_access_key)?;

            let mut state = self.state.lock().unwrap();
            let mut next = state.ws.clone();
            let profile_id = profile_id_for(&mut next, profile);
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

/// The checked fields of a create or edit request. A key is `None` only if the request gave
/// none (edit: keep the old key).
struct Valid<'a> {
    name: &'a str,
    keys: [Option<&'a str>; 2],
    profile: &'a str,
    mfa_device: Option<String>,
}

/// The validation of `create_iam_user` and `update_iam_user`, in the order of the form fields.
/// A key given as `None` is not checked; a key given as `Some` must not be empty.
fn validate<'a>(
    name: &'a str,
    keys: [(Field, Option<&'a str>); 2],
    profile: &'a str,
    region: &str,
    mfa_device: Option<&str>,
) -> Result<Valid<'a>> {
    let name = required(Field::Name, name)?;
    let [(f0, k0), (f1, k1)] = keys;
    let keys = [
        k0.map(|k| required(f0, k)).transpose()?,
        k1.map(|k| required(f1, k)).transpose()?,
    ];
    let profile = required(Field::Profile, profile)?;
    check_profile_chars(profile)?;
    check_region(region)?;
    let mfa_device = mfa_device
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    Ok(Valid {
        name,
        keys,
        profile,
        mfa_device,
    })
}

/// The F0 rule for a profile name: it goes in a `[section]` header of the credentials file.
fn check_profile_chars(profile: &str) -> Result<()> {
    if profile.contains(['[', ']', '\n', '\r']) {
        return Err(Error::InvalidInput {
            field: Field::Profile,
            message: "must not contain [, ], or line breaks".into(),
        });
    }
    Ok(())
}

fn check_region(region: &str) -> Result<()> {
    if !regions::REGIONS.contains(&region) {
        return Err(Error::InvalidInput {
            field: Field::Region,
            message: format!("unknown region \"{region}\""),
        });
    }
    Ok(())
}

/// The id of the named profile `name`. Adds the profile to `ws` if it is not there.
fn profile_id_for(ws: &mut Workspace, name: &str) -> Uuid {
    if let Some(p) = ws.profiles.iter().find(|p| p.name == name) {
        return p.id;
    }
    let p = NamedProfile {
        id: Uuid::new_v4(),
        name: name.to_string(),
    };
    let id = p.id;
    ws.profiles.push(p);
    id
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
