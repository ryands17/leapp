//! A headless harness: the real `Ui` in a Rune `Runtime`, on a temp home, `MemoryStore` and
//! `FakeAws`.

#![allow(dead_code)]

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fake_aws::FakeAws;
use leapp_app::{Bridge, Ui, UiPrompter};
use leapp_core::{Config, Leapp, MemoryStore, NewIamUser};
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use tempfile::TempDir;

pub const SIZE: (f32, f32) = (1100.0, 700.0);

pub struct AppHarness {
    pub home: TempDir,
    pub fake: FakeAws,
    pub secrets: Arc<MemoryStore>,
    pub leapp: Arc<Leapp>,
    /// A clone of the Ui's bridge, so tests can send messages.
    pub bridge: Bridge,
    pub rt: Runtime<Ui>,
    tokio: tokio::runtime::Runtime,
}

impl AppHarness {
    pub fn new() -> AppHarness {
        Self::with_setup(|_| ()).0
    }

    /// Like `new`, but runs `before` on the `Leapp` before the `Ui` reads the workspace.
    pub fn with_setup<T>(before: impl FnOnce(&Leapp) -> T) -> (AppHarness, T) {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let home = TempDir::new().unwrap();
        let fake = tokio.block_on(FakeAws::start());
        let secrets = Arc::new(MemoryStore::default());
        let bridge = Bridge::new();
        let prompter = Arc::new(UiPrompter::new(bridge.clone()));
        let mut config = Config::new(home.path().to_path_buf(), secrets.clone(), prompter);
        config.endpoints.sts = Some(fake.url());
        let leapp = Arc::new(Leapp::open(config).unwrap());
        let out = before(&leapp);

        let ui = Ui::new(leapp.clone(), tokio.handle().clone(), bridge.clone());
        let mut rt = Runtime::new(ui, leapp_app::view, SIZE, Env::new(false, 1.0));
        rt.poll = Some(leapp_app::poll);
        let mut h = AppHarness {
            home,
            fake,
            secrets,
            leapp,
            bridge,
            rt,
            tokio,
        };
        h.frames(1);
        (h, out)
    }

    /// Runs `f` to the end on the harness's tokio runtime.
    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.tokio.block_on(f)
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.rt.frame(1.0 / 60.0);
        }
    }

    /// Runs frames for the arming delay of the overwrite question (harness time), so that the
    /// question in front takes an answer.
    pub fn wait_for_arming(&mut self) {
        let n = (leapp_app::CONFIRM_ARM_SECONDS * 60.0).ceil() as usize + 2;
        self.frames(n);
    }

    /// Runs frames until `until` is true. Panics after 5 s of real time.
    pub fn settle(&mut self, until: impl Fn(&Ui) -> bool) {
        self.settle_with(|h| until(&h.rt.state));
    }

    /// Like `settle`, but `until` sees the whole harness (the element tree too).
    pub fn settle_with(&mut self, until: impl Fn(&AppHarness) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.frames(1);
            if until(self) {
                // One more frame, so the view shows the new state.
                self.frames(1);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the UI did not settle in 5 s; sessions: {:?}, questions: {}",
                self.rt
                    .state
                    .workspace()
                    .sessions
                    .iter()
                    .map(|s| s.status)
                    .collect::<Vec<_>>(),
                self.rt.state.pending_confirm_count()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Presses and releases the pointer on the element `key`.
    pub fn tap(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element with key {key:?}"));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.frames(2);
    }

    pub fn type_text(&mut self, text: &str) {
        self.rt.key(KeyEvent {
            key: Key::Text(text.to_string()),
            mods: Mods::default(),
        });
        self.frames(1);
    }

    /// The text of the element `key`, or its label if it has no text, or else the text of its
    /// first child that has text.
    pub fn text_of(&self, key: &str) -> Option<String> {
        let elements = self.rt.elements();
        let el = elements
            .iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.exiting)?;
        if let Some(t) = el.text.clone().or_else(|| el.label.clone()) {
            return Some(t);
        }
        let prefix = format!("{}/", el.path);
        elements
            .iter()
            .filter(|e| e.path.starts_with(&prefix))
            .find_map(|e| e.text.clone())
    }

    pub fn has(&self, key: &str) -> bool {
        self.rt.find(key).is_some()
    }

    /// The drawn rect `[x, y, w, h]` of the element `key`.
    pub fn rect(&self, key: &str) -> [f32; 4] {
        self.rt
            .presented(key)
            .unwrap_or_else(|| panic!("no element with key {key:?}"))
            .0
    }

    /// The keys of all elements whose key starts with `prefix`, top to bottom.
    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        let mut found: Vec<_> = self
            .rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| Some((e.key?, e.rect[1])))
            .filter(|(k, _)| k.starts_with(prefix))
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found.into_iter().map(|(k, _)| k).collect()
    }
}

/// An IAM User in `eu-west-1` with fake keys.
pub fn iam_user(name: &str, profile: &str) -> NewIamUser {
    NewIamUser {
        name: name.to_string(),
        access_key_id: format!("AKID-{name}"),
        secret_access_key: format!("SECRET-{name}"),
        profile_name: profile.to_string(),
        region: "eu-west-1".to_string(),
    }
}
