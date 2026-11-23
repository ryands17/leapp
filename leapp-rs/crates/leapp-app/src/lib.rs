//! The Leapp RS desktop UI: state, view and the bridge to `leapp-core`.

mod bridge;
mod views;

use std::future::Future;
use std::sync::{Arc, OnceLock};

use leapp_core::{Leapp, NewIamUser, Workspace};
use rune::{AppSpec, Cx, El, Tx, Waker};
use rune_kit::{Kit, root};
use tokio::sync::oneshot;

pub use bridge::{Bridge, Msg, UiPrompter};
pub use views::create_dialog::CreateForm;

/// A question from core that waits for the user's answer.
struct PendingConfirm {
    profile: String,
    #[expect(dead_code, reason = "Task 7 sends the answer")]
    reply: oneshot::Sender<bool>,
}

/// The state of the UI. The render thread owns it.
pub struct Ui {
    leapp: Arc<Leapp>,
    rt: tokio::runtime::Handle,
    bridge: Bridge,
    ws: Workspace,
    error: Option<String>,
    create: CreateForm,
    /// Task 7 shows this as a dialog.
    confirm: Option<PendingConfirm>,
}

impl Ui {
    pub fn new(leapp: Arc<Leapp>, rt: tokio::runtime::Handle, bridge: Bridge) -> Ui {
        let ws = leapp.workspace();
        Ui {
            leapp,
            rt,
            bridge,
            ws,
            error: None,
            create: CreateForm::default(),
            confirm: None,
        }
    }

    /// The workspace as the UI shows it now.
    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// The error the UI shows now, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The Create IAM User dialog.
    pub fn create_form(&self) -> &CreateForm {
        &self.create
    }

    /// The profile of the overwrite question that waits for an answer, if any.
    pub fn pending_confirm(&self) -> Option<&str> {
        self.confirm.as_ref().map(|c| c.profile.as_str())
    }

    /// Runs `f` on the tokio runtime, off the render thread. When it ends, the UI reads the
    /// workspace again; if it fails, the UI also shows the error.
    pub fn run<F>(&self, f: F)
    where
        F: Future<Output = leapp_core::Result<()>> + Send + 'static,
    {
        let bridge = self.bridge.clone();
        self.rt.spawn(async move {
            match f.await {
                Ok(()) => bridge.send(Msg::Refreshed),
                Err(e) => bridge.send(Msg::Failed(e.to_string())),
            }
        });
    }

    fn refresh(&mut self) {
        self.ws = self.leapp.workspace();
    }
}

pub fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let mut layer = Vec::new();
    views::create_dialog::build(s, &k, &mut layer);
    root(views::main_window::view(s, &k), layer)
}

/// Reads all bridge messages. True if the state changed.
pub fn poll(s: &mut Ui, _t: f64, _tx: &mut Tx) -> bool {
    let mut changed = false;
    for m in s.bridge.drain() {
        changed = true;
        match m {
            Msg::Refreshed => s.refresh(),
            Msg::Failed(message) => {
                s.error = Some(message);
                s.refresh();
            }
            Msg::ConfirmOverwrite { profile, reply } => {
                s.confirm = Some(PendingConfirm { profile, reply });
            }
        }
    }
    changed
}

/// Gives the bridge the waker of the UI thread.
pub fn setup(s: &mut Ui, waker: Waker) {
    s.bridge.set_waker(waker);
}

/// What `spec().init` builds the [`Ui`] from.
struct Context {
    leapp: Arc<Leapp>,
    rt: tokio::runtime::Handle,
    bridge: Bridge,
}

static CONTEXT: OnceLock<Context> = OnceLock::new();

/// Sets what `spec().init` builds the [`Ui`] from. Call it one time, before `rune::run`.
/// Returns false if it was set before (the first values stay).
pub fn install(leapp: Arc<Leapp>, rt: tokio::runtime::Handle, bridge: Bridge) -> bool {
    CONTEXT.set(Context { leapp, rt, bridge }).is_ok()
}

fn init() -> Ui {
    let c = CONTEXT
        .get()
        .expect("call leapp_app::install before rune::run");
    Ui::new(c.leapp.clone(), c.rt.clone(), c.bridge.clone())
}

/// A named change to the first state, for headless snapshots (`--scene NAME`).
type Scene = (&'static str, fn(&mut Ui));

const SCENES: &[Scene] = &[("one-session", one_session)];

/// One inactive IAM User. Needs no AWS: it only writes the workspace and the secret store.
fn one_session(s: &mut Ui) {
    let created = s.leapp.create_iam_user(NewIamUser {
        name: "My IAM User".into(),
        access_key_id: "AKIAEXAMPLE".into(),
        secret_access_key: "example-secret".into(),
        profile_name: "default".into(),
        region: "us-east-1".into(),
    });
    if let Err(e) = created {
        s.error = Some(e.to_string());
    }
    s.refresh();
}

pub fn spec() -> AppSpec<Ui> {
    AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        scenes: SCENES,
        ..AppSpec::new("Leapp RS", (1100.0, 700.0), init, view)
    }
}
