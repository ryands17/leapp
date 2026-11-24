#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use async_trait::async_trait;
use fake_aws::FakeAws;
use leapp_core::{
    Clock, Config, Error, Leapp, MemoryStore, NewIamUser, Prompter, Result, SecretStore,
};
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

/// A `MemoryStore` that can fail a chosen `set` and hold `get` calls until a gate opens.
#[derive(Default)]
pub struct FlakyStore {
    pub inner: Arc<MemoryStore>,
    /// Number of `set` calls that still succeed before one fails. `None`: no failure.
    sets_before_failure: Mutex<Option<usize>>,
    gates: Mutex<Vec<(String, Gate)>>,
}

impl FlakyStore {
    /// The `(n+1)`-th `set` from now fails with `Error::Secret`. Later sets work again.
    pub fn fail_set_after(&self, n: usize) {
        *self.sets_before_failure.lock().unwrap() = Some(n);
    }

    /// A `get` of a key that starts with `key_prefix` waits until the gate opens.
    pub fn block_get(&self, key_prefix: &str) -> Gate {
        let gate = Gate::default();
        self.gates
            .lock()
            .unwrap()
            .push((key_prefix.to_string(), gate.clone()));
        gate
    }
}

impl SecretStore for FlakyStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let gates: Vec<Gate> = self
            .gates
            .lock()
            .unwrap()
            .iter()
            .filter(|(prefix, _)| key.starts_with(prefix.as_str()))
            .map(|(_, g)| g.clone())
            .collect();
        for gate in gates {
            gate.pass();
        }
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        {
            let mut left = self.sets_before_failure.lock().unwrap();
            match *left {
                Some(0) => {
                    *left = None;
                    return Err(Error::Secret("the test store refused the write".into()));
                }
                Some(n) => *left = Some(n - 1),
                None => {}
            }
        }
        self.inner.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key)
    }
}

/// Holds the `get` calls of a `FlakyStore` until `open`.
#[derive(Clone, Default)]
pub struct Gate(Arc<(Mutex<GateState>, Condvar)>);

#[derive(Default)]
struct GateState {
    open: bool,
    waiting: usize,
}

impl Gate {
    pub fn open(&self) {
        let (state, cv) = &*self.0;
        state.lock().unwrap().open = true;
        cv.notify_all();
    }

    /// Waits until a `get` waits at this gate. Panics after 5 s.
    pub fn wait_blocked(&self) {
        let (state, cv) = &*self.0;
        let guard = state.lock().unwrap();
        let (guard, _) = cv
            .wait_timeout_while(guard, std::time::Duration::from_secs(5), |s| s.waiting == 0)
            .unwrap();
        assert!(guard.waiting > 0, "no get came to the gate");
    }

    fn pass(&self) {
        let (state, cv) = &*self.0;
        let mut guard = state.lock().unwrap();
        guard.waiting += 1;
        cv.notify_all();
        let mut guard = cv.wait_while(guard, |s| !s.open).unwrap();
        guard.waiting -= 1;
    }
}

pub struct Harness {
    pub home: TempDir,
    /// The keys and values the `Leapp` stored. With `with_store`, the inner store of the
    /// `FlakyStore`.
    pub secrets: Arc<MemoryStore>,
    /// The store that the `Leapp` uses.
    store: Arc<dyn SecretStore>,
    pub prompter: Arc<ScriptedPrompter>,
    pub fake: FakeAws,
    pub leapp: Leapp,
}

impl Harness {
    pub async fn new() -> Harness {
        let secrets = Arc::new(MemoryStore::default());
        Self::build(secrets.clone(), secrets, |_| None).await
    }

    /// Like `new`, with a `FlakyStore` as the secret store.
    pub async fn with_store(store: Arc<FlakyStore>) -> Harness {
        Self::build(store.inner.clone(), store, |_| None).await
    }

    /// Like `new`, with a clock chosen from the temp home folder. The clock is the only hook that
    /// runs between the file write and the last workspace save of `start`.
    pub async fn with_clock(make: impl FnOnce(&Path) -> Arc<dyn Clock>) -> Harness {
        let secrets = Arc::new(MemoryStore::default());
        Self::build(secrets.clone(), secrets, |home| Some(make(home))).await
    }

    async fn build(
        secrets: Arc<MemoryStore>,
        store: Arc<dyn SecretStore>,
        clock: impl FnOnce(&Path) -> Option<Arc<dyn Clock>>,
    ) -> Harness {
        let home = TempDir::new().unwrap();
        let fake = FakeAws::start().await;
        let prompter = Arc::new(ScriptedPrompter {
            answer: AtomicBool::new(true),
            asked: Mutex::new(Vec::new()),
        });
        let mut config = Self::config(&home, &store, &prompter, &fake);
        if let Some(clock) = clock(home.path()) {
            config.clock = clock;
        }
        let leapp = Leapp::open(config).unwrap();
        Harness {
            home,
            secrets,
            store,
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
            store,
            prompter,
            fake,
            leapp,
        } = self;
        drop(leapp);
        let leapp = Leapp::open(Self::config(&home, &store, &prompter, &fake)).unwrap();
        Harness {
            home,
            secrets,
            store,
            prompter,
            fake,
            leapp,
        }
    }

    fn config(
        home: &TempDir,
        store: &Arc<dyn SecretStore>,
        prompter: &Arc<ScriptedPrompter>,
        fake: &FakeAws,
    ) -> Config {
        let mut config = Config::new(home.path().to_path_buf(), store.clone(), prompter.clone());
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
