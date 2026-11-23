use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::secrets::SecretStore;

#[derive(Clone, Default)]
pub struct Endpoints {
    /// `None` means real AWS.
    pub sts: Option<String>,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[async_trait]
pub trait Prompter: Send + Sync {
    async fn confirm_profile_overwrite(&self, profile: &str) -> bool;
}

pub struct Config {
    pub home_dir: PathBuf,
    /// Default: `home_dir/.leapp-rs`.
    pub data_dir: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub endpoints: Endpoints,
    pub clock: Arc<dyn Clock>,
    pub prompter: Arc<dyn Prompter>,
}

impl Config {
    /// Real endpoints and the system clock.
    pub fn new(
        home_dir: PathBuf,
        secrets: Arc<dyn SecretStore>,
        prompter: Arc<dyn Prompter>,
    ) -> Config {
        Config {
            data_dir: home_dir.join(".leapp-rs"),
            home_dir,
            secrets,
            endpoints: Endpoints::default(),
            clock: Arc::new(SystemClock),
            prompter,
        }
    }
}
