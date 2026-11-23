#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fake_aws::FakeAws;
use leapp_core::{Clock, Config, Leapp, MemoryStore, NewIamUser, Prompter};
use tempfile::TempDir;

pub struct ScriptedPrompter {
    answer: AtomicBool,
    asked: Mutex<Vec<String>>,
}

impl ScriptedPrompter {
    pub fn answer(&self, yes: bool) {
        self.answer.store(yes, Ordering::SeqCst);
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl Prompter for ScriptedPrompter {
    async fn confirm_profile_overwrite(&self, profile: &str) -> bool {
        self.asked.lock().unwrap().push(profile.to_string());
        self.answer.load(Ordering::SeqCst)
    }
}

pub struct Harness {
    pub home: TempDir,
    pub secrets: Arc<MemoryStore>,
    pub prompter: Arc<ScriptedPrompter>,
    pub fake: FakeAws,
    pub leapp: Leapp,
}

impl Harness {
    pub async fn new() -> Harness {
        Self::build(|_| None).await
    }

    /// Like `new`, with a clock chosen from the temp home folder. The clock is the only hook that
    /// runs between the file write and the last workspace save of `start`.
    pub async fn with_clock(make: impl FnOnce(&Path) -> Arc<dyn Clock>) -> Harness {
        Self::build(|home| Some(make(home))).await
    }

    async fn build(clock: impl FnOnce(&Path) -> Option<Arc<dyn Clock>>) -> Harness {
        let home = TempDir::new().unwrap();
        let fake = FakeAws::start().await;
        let secrets = Arc::new(MemoryStore::default());
        let prompter = Arc::new(ScriptedPrompter {
            answer: AtomicBool::new(true),
            asked: Mutex::new(Vec::new()),
        });
        let mut config = Self::config(&home, &secrets, &prompter, &fake);
        if let Some(clock) = clock(home.path()) {
            config.clock = clock;
        }
        let leapp = Leapp::open(config).unwrap();
        Harness {
            home,
            secrets,
            prompter,
            fake,
            leapp,
        }
    }

    /// Drops the `Leapp` without `shutdown` and opens a new one on the same folders.
    pub fn reopen(self) -> Harness {
        let Harness {
            home,
            secrets,
            prompter,
            fake,
            leapp,
        } = self;
        drop(leapp);
        let leapp = Leapp::open(Self::config(&home, &secrets, &prompter, &fake)).unwrap();
        Harness {
            home,
            secrets,
            prompter,
            fake,
            leapp,
        }
    }

    fn config(
        home: &TempDir,
        secrets: &Arc<MemoryStore>,
        prompter: &Arc<ScriptedPrompter>,
        fake: &FakeAws,
    ) -> Config {
        let mut config = Config::new(home.path().to_path_buf(), secrets.clone(), prompter.clone());
        config.endpoints.sts = Some(fake.url());
        config
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.home.path().join(".aws").join("credentials")
    }

    pub fn workspace_path(&self) -> PathBuf {
        self.home.path().join(".leapp-rs").join("workspace.json")
    }

    pub fn credentials_text(&self) -> Option<String> {
        std::fs::read_to_string(self.credentials_path()).ok()
    }

    pub fn write_credentials(&self, text: &str) {
        let path = self.credentials_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub fn workspace_text(&self) -> String {
        std::fs::read_to_string(self.workspace_path()).unwrap()
    }

    pub fn iam_user(&self, name: &str, profile: &str) -> NewIamUser {
        NewIamUser {
            name: name.to_string(),
            access_key_id: format!("AKID-{name}"),
            secret_access_key: format!("SECRET-{name}"),
            profile_name: profile.to_string(),
            region: "eu-west-1".to_string(),
        }
    }
}

/// A clock that runs a hook on every call. Tests use it to break the disk at a chosen moment.
pub struct HookClock<F: Fn() + Send + Sync>(pub F);

impl<F: Fn() + Send + Sync> Clock for HookClock<F> {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.0)();
        chrono::Utc::now()
    }
}

/// Sets the mode of a folder. Unix only; the tests that use it are `#[cfg(unix)]`.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
