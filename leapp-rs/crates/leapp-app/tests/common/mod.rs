//! A headless harness: the real `Ui` in a Rune `Runtime`, on a temp home, `MemoryStore` and
//! `FakeAws`.

#![allow(dead_code)]

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use fake_aws::FakeAws;
use leapp_app::{Bridge, Ui, UiPrompter};
use leapp_core::SessionId;
use leapp_core::{Clock, Config, Error, Leapp, MemoryStore, NewIamUser, SecretStore};
use rune::clipboard::MemoryClipboard;
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use tempfile::TempDir;

pub const SIZE: (f32, f32) = (1100.0, 700.0);

/// The worker threads of the harness's tokio runtime.
const WORKERS: usize = 2;

pub struct AppHarness {
    pub home: TempDir,
    pub fake: FakeAws,
    /// What the secret store holds. The same map as `store.inner`.
    pub secrets: Arc<MemoryStore>,
    /// The secret store of the `Leapp`: `secrets`, and a write can be made to fail.
    pub store: Arc<FlakyStore>,
    pub leapp: Arc<Leapp>,
    /// The clock of the `Leapp`. It moves only when a test advances it.
    pub clock: Arc<ManualClock>,
    /// A clone of the Ui's bridge, so tests can send messages.
    pub bridge: Bridge,
    pub rt: Runtime<Ui>,
    /// The clipboard of `rt`: in memory, never the OS clipboard.
    clipboard: MemoryClipboard,
    tokio: tokio::runtime::Runtime,
}

impl AppHarness {
    pub fn new() -> AppHarness {
        Self::with_setup(|_| ()).0
    }

    /// Like `new`, but runs `before` on the `Leapp` before the `Ui` reads the workspace.
    pub fn with_setup<T>(before: impl FnOnce(&Leapp) -> T) -> (AppHarness, T) {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .enable_all()
            .build()
            .unwrap();
        let home = TempDir::new().unwrap();
        let fake = tokio.block_on(FakeAws::start());
        let store = Arc::new(FlakyStore::default());
        let secrets = store.inner.clone();
        let bridge = Bridge::new();
        let prompter = Arc::new(UiPrompter::new(bridge.clone()));
        let clock = Arc::new(ManualClock::default());
        let mut config = Config::new(home.path().to_path_buf(), store.clone(), prompter);
        config.endpoints.sts = Some(fake.url());
        config.clock = clock.clone();
        let leapp = Arc::new(Leapp::open(config).unwrap());
        let out = before(&leapp);

        let ui = Ui::new(leapp.clone(), tokio.handle().clone(), bridge.clone());
        let mut rt = Runtime::new(ui, leapp_app::view, SIZE, Env::new(false, 1.0));
        rt.poll = Some(leapp_app::poll);
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        let mut h = AppHarness {
            home,
            fake,
            secrets,
            store,
            leapp,
            clock,
            bridge,
            rt,
            clipboard,
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

    /// Runs `f` on the harness's tokio runtime and returns its handle, so a test can wait for
    /// the end of a task that the UI does not show.
    pub fn spawn<F>(&self, f: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tokio.spawn(f)
    }

    /// Starts the app's refresh loop on the harness's tokio runtime, with a tick of 10 ms.
    pub fn spawn_refresh_loop(&self) -> tokio::task::JoinHandle<()> {
        leapp_app::spawn_refresh_loop(
            self.leapp.clone(),
            self.tokio.handle(),
            self.bridge.clone(),
            Duration::from_millis(10),
        )
    }

    /// Blocks every tokio worker until the gate opens (or 10 s), so that no task of the UI or
    /// of core runs in the meantime. Returns when all workers are blocked.
    pub fn hold_workers(&self) -> Gate {
        let gate = Gate::default();
        let blocked = Arc::new((Mutex::new(0usize), Condvar::new()));
        for _ in 0..WORKERS {
            let (gate, blocked) = (gate.clone(), blocked.clone());
            self.tokio.spawn(async move {
                *blocked.0.lock().unwrap() += 1;
                blocked.1.notify_all();
                gate.pass();
            });
        }
        let count = blocked.0.lock().unwrap();
        let (count, timeout) = blocked
            .1
            .wait_timeout_while(count, Duration::from_secs(5), |n| *n < WORKERS)
            .unwrap();
        assert!(!timeout.timed_out(), "only {} workers blocked", *count);
        gate
    }

    /// Runs frames for the arming delay of the question (harness time), so that the
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
                self.rt.state.pending_question_count()
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

    /// A secondary press (right-click) on the element `key`.
    pub fn secondary_tap(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element with key {key:?}"));
        self.rt.secondary_down(x, y);
        self.rt.secondary_up(x, y);
        self.frames(2);
    }

    /// Taps the item of the open row menu of session `id` whose text is `label`.
    pub fn tap_menu_item(&mut self, id: SessionId, label: &str) {
        let prefix = format!("menu-{id}-item");
        let key = self
            .keys_with_prefix(&prefix)
            .into_iter()
            .find(|k| self.text_of(k).as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no menu item {label:?} under {prefix:?}"));
        self.tap(&key);
    }

    /// The text on the clipboard of the UI, if any.
    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.text().map(|t| t.as_str().to_string())
    }

    /// Presses one key, such as `Key::Enter`, on the focused element.
    pub fn press(&mut self, key: Key) {
        self.rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
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

    /// True if a shown text element has exactly `text`.
    pub fn has_text(&self, text: &str) -> bool {
        self.rt
            .elements()
            .iter()
            .any(|e| !e.exiting && e.text.as_deref() == Some(text))
    }

    /// True if any element of the tree shows `needle` in its text or label.
    pub fn tree_contains(&self, needle: &str) -> bool {
        self.rt.elements().iter().any(|e| {
            e.text.as_deref().is_some_and(|t| t.contains(needle))
                || e.label.as_deref().is_some_and(|t| t.contains(needle))
        })
    }

    /// True if the accessibility tree marks the element `key` as disabled (rune-kit `Button`
    /// disabled state).
    pub fn is_disabled(&self, key: &str) -> bool {
        fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
            if v["key"] == key {
                return Some(v);
            }
            v["children"].as_array()?.iter().find_map(|c| find(c, key))
        }
        let tree = self.rt.accessibility_json();
        let node = find(&tree, key).unwrap_or_else(|| panic!("no node keyed {key:?}"));
        node["states"]
            .as_array()
            .is_some_and(|s| s.iter().any(|v| v == "disabled"))
    }

    /// The placeholder of the text field inside the element `key`, from the accessibility tree.
    pub fn placeholder_of(&self, key: &str) -> Option<String> {
        fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
            if v["key"] == key {
                return Some(v);
            }
            v["children"].as_array()?.iter().find_map(|c| find(c, key))
        }
        fn placeholder(v: &serde_json::Value) -> Option<String> {
            if let Some(p) = v["placeholder"].as_str() {
                return Some(p.to_string());
            }
            v["children"].as_array()?.iter().find_map(placeholder)
        }
        let tree = self.rt.accessibility_json();
        placeholder(find(&tree, key)?)
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

    /// The keys of the session rows (`row-<id>`), top to bottom. Not their `row-menu-<id>`
    /// buttons.
    pub fn row_keys(&self) -> Vec<String> {
        self.keys_with_prefix("row-")
            .into_iter()
            .filter(|k| !k.starts_with("row-menu-"))
            .collect()
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
        mfa_device: None,
    }
}

/// Like [`iam_user`], with an MFA device.
pub fn mfa_iam_user(name: &str, profile: &str) -> NewIamUser {
    NewIamUser {
        mfa_device: Some(MFA_DEVICE.to_string()),
        ..iam_user(name, profile)
    }
}

pub const MFA_DEVICE: &str = "arn:aws:iam::123456789012:mfa/dev";

/// A clock that starts at the real time and moves only when a test advances it.
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock(Mutex::new(Utc::now()))
    }
}

impl ManualClock {
    pub fn advance(&self, by: chrono::Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// A `MemoryStore` whose next `set` can be made to fail.
#[derive(Default)]
pub struct FlakyStore {
    pub inner: Arc<MemoryStore>,
    fail_next_set: Mutex<bool>,
    hold: Mutex<Option<Gate>>,
    /// The next `get` of this key panics.
    panic_on_get: Mutex<Option<String>>,
}

/// Holds the `set` calls of a `FlakyStore` until `open`.
#[derive(Clone, Default)]
pub struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    pub fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }

    /// Waits until the gate opens, or 10 s, so that a failed test does not hang the runtime.
    fn pass(&self) {
        let open = self.0.0.lock().unwrap();
        let _ = self
            .0
            .1
            .wait_timeout_while(open, Duration::from_secs(10), |open| !*open)
            .unwrap();
    }
}

impl FlakyStore {
    /// The next `set` fails with `Error::Secret`. Later sets work again.
    pub fn fail_next_set(&self) {
        *self.fail_next_set.lock().unwrap() = true;
    }

    /// The next `get` of `key` panics, one time.
    pub fn panic_on_next_get(&self, key: &str) {
        *self.panic_on_get.lock().unwrap() = Some(key.to_string());
    }

    /// Every `set` from now waits until the gate opens.
    pub fn hold_sets(&self) -> Gate {
        let gate = Gate::default();
        *self.hold.lock().unwrap() = Some(gate.clone());
        gate
    }
}

impl SecretStore for FlakyStore {
    fn get(&self, key: &str) -> leapp_core::Result<Option<String>> {
        let panic = {
            let mut armed = self.panic_on_get.lock().unwrap();
            if armed.as_deref() == Some(key) {
                armed.take()
            } else {
                None
            }
        };
        // Panic outside the lock, so the store stays usable.
        if panic.is_some() {
            panic!("the test store panics on a get");
        }
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: &str) -> leapp_core::Result<()> {
        let gate = self.hold.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass();
        }
        if std::mem::take(&mut *self.fail_next_set.lock().unwrap()) {
            return Err(Error::Secret("the test store refused the write".into()));
        }
        self.inner.set(key, value)
    }

    fn delete(&self, key: &str) -> leapp_core::Result<()> {
        self.inner.delete(key)
    }
}
