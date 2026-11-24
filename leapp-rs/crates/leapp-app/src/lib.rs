//! The Leapp RS desktop UI: state, view and the bridge to `leapp-core`.

mod bridge;
mod views;

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use leapp_core::{Leapp, NewIamUser, SessionId, SessionStatus, Workspace};
use rune::{AppSpec, Cx, El, TextEdit, Tx, Waker};
use rune_kit::{Kit, root};
use tokio::sync::oneshot;

pub use bridge::{Bridge, Msg, UiPrompter};
pub use views::create_dialog::CreateForm;

/// A question from core that waits for the user's answer. Dropping it without an answer means
/// no (overwrite) or cancel (MFA).
enum Question {
    /// Can core overwrite the user's named profile?
    Overwrite {
        profile: String,
        reply: oneshot::Sender<bool>,
    },
    /// The MFA code for the session `session` (its name). `None` cancels.
    Mfa {
        session: String,
        reply: oneshot::Sender<Option<String>>,
    },
}

/// True if `code`, without the spaces around it, is exactly 6 ASCII digits.
fn is_valid_mfa_code(code: &str) -> bool {
    let code = code.trim();
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

/// An empty, secure field for the MFA code. `TextEdit` wipes its buffers when it drops.
fn empty_mfa_code() -> TextEdit {
    TextEdit::default().secure()
}

/// How long the error toast stays, in seconds.
const TOAST_SECONDS: f64 = 8.0;

/// For how long, in seconds, a question ignores answers after it comes to the front.
/// The second click of a double-click on "Overwrite" must not answer the next question.
pub const CONFIRM_ARM_SECONDS: f64 = 0.5;

/// The state of the UI. The render thread owns it.
pub struct Ui {
    leapp: Arc<Leapp>,
    rt: tokio::runtime::Handle,
    bridge: Bridge,
    ws: Workspace,
    error: Option<String>,
    /// The clock of `poll` when the toast first showed. `None` until the next `poll`.
    error_since: Option<f64>,
    create: CreateForm,
    /// Questions from core, oldest first. A dialog shows the first one.
    questions: VecDeque<Question>,
    /// The clock of `poll` when the first question came to the front. `None` until the next
    /// `poll`.
    question_since: Option<f64>,
    /// True when the first question takes answers: [`CONFIRM_ARM_SECONDS`] after
    /// `question_since`.
    question_armed: bool,
    /// The field of the MFA dialog. Secure. A new MFA question at the front gets a new, empty one.
    mfa_code: TextEdit,
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
            error_since: None,
            create: CreateForm::default(),
            questions: VecDeque::new(),
            question_since: None,
            question_armed: false,
            mfa_code: empty_mfa_code(),
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

    /// The profile of the overwrite question that the dialog shows now, if any.
    pub fn pending_confirm(&self) -> Option<&str> {
        match self.questions.front() {
            Some(Question::Overwrite { profile, .. }) => Some(profile),
            _ => None,
        }
    }

    /// The session name of the MFA question that the dialog shows now, if any.
    pub fn pending_mfa(&self) -> Option<&str> {
        match self.questions.front() {
            Some(Question::Mfa { session, .. }) => Some(session),
            _ => None,
        }
    }

    /// How many questions wait for an answer, the open one included.
    pub fn pending_question_count(&self) -> usize {
        self.questions.len()
    }

    /// True when the question in front takes answers.
    pub(crate) fn question_armed(&self) -> bool {
        self.question_armed
    }

    /// True when the MFA dialog can submit its code: armed, and the code is 6 digits.
    pub(crate) fn can_submit_mfa(&self) -> bool {
        self.question_armed && is_valid_mfa_code(&self.mfa_code.text)
    }

    /// Takes the question in front off the line. Later questions stay in line; the next one
    /// starts its own arming delay at the next `poll`.
    fn pop_question(&mut self) -> Option<Question> {
        let q = self.questions.pop_front()?;
        self.question_since = None;
        self.question_armed = false;
        Some(q)
    }

    /// Answers the overwrite question in front. Ignored if the front question is not an
    /// overwrite question, or if it is not armed: an answer in the first
    /// [`CONFIRM_ARM_SECONDS`] after it came to the front is ignored.
    pub(crate) fn answer_confirm(&mut self, yes: bool) {
        if !self.question_armed || self.pending_confirm().is_none() {
            return;
        }
        if let Some(Question::Overwrite { reply, .. }) = self.pop_question() {
            // The task of core can end before the answer: then nobody waits for it.
            let _ = reply.send(yes);
        }
    }

    /// Sends the code of the MFA dialog, trimmed. Ignored unless [`Ui::can_submit_mfa`].
    pub(crate) fn submit_mfa(&mut self) {
        if !self.can_submit_mfa() || self.pending_mfa().is_none() {
            return;
        }
        let code = self.mfa_code.text.trim().to_string();
        self.mfa_code = empty_mfa_code();
        if let Some(Question::Mfa { reply, .. }) = self.pop_question() {
            let _ = reply.send(Some(code));
        }
    }

    /// Cancels the MFA question in front. Ignored until it is armed.
    pub(crate) fn cancel_mfa(&mut self) {
        if !self.question_armed || self.pending_mfa().is_none() {
            return;
        }
        self.mfa_code = empty_mfa_code();
        if let Some(Question::Mfa { reply, .. }) = self.pop_question() {
            let _ = reply.send(None);
        }
    }

    pub(crate) fn mfa_code(&self) -> &TextEdit {
        &self.mfa_code
    }

    pub(crate) fn mfa_code_mut(&mut self) -> &mut TextEdit {
        &mut self.mfa_code
    }

    /// Shows `message` in the error toast. The toast hides after [`TOAST_SECONDS`].
    pub(crate) fn show_error(&mut self, message: String) {
        self.error = Some(message);
        self.error_since = None;
    }

    pub(crate) fn clear_error(&mut self) {
        self.error = None;
        self.error_since = None;
    }

    /// Starts an inactive session or stops an active one. A pending session stays as it is.
    /// A start shows `Pending` at once; core confirms it, or sets `Inactive` if the start fails.
    pub(crate) fn toggle(&mut self, id: SessionId) {
        let Some(status) = self.ws.session(id).map(|s| s.status) else {
            return;
        };
        match status {
            SessionStatus::Pending => {}
            SessionStatus::Inactive => {
                if let Some(s) = self.ws.sessions.iter_mut().find(|s| s.id == id) {
                    s.status = SessionStatus::Pending;
                }
                let leapp = self.leapp.clone();
                self.run(async move { leapp.start(id).await });
            }
            SessionStatus::Active => {
                let leapp = self.leapp.clone();
                self.run(async move { leapp.stop(id).await });
            }
        }
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
    views::overwrite_dialog::build(s, &k, &mut layer);
    views::mfa_dialog::build(s, &k, &mut layer);
    root(views::main_window::view(s, &k), layer)
}

/// Reads all bridge messages. True if the state changed.
pub fn poll(s: &mut Ui, t: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    for m in s.bridge.drain() {
        changed = true;
        match m {
            Msg::Refreshed => s.refresh(),
            Msg::Failed(message) => {
                s.show_error(message);
                s.refresh();
            }
            Msg::ConfirmOverwrite { profile, reply } => {
                s.questions
                    .push_back(Question::Overwrite { profile, reply });
            }
            Msg::AskMfa { session, reply } => {
                s.questions.push_back(Question::Mfa { session, reply });
            }
        }
    }
    if !s.questions.is_empty() && !s.question_armed {
        let since = match s.question_since {
            Some(since) => since,
            None => {
                // The question just came to the front.
                s.question_since = Some(t);
                if s.pending_mfa().is_some() {
                    // A new, empty field that takes the keys at once.
                    s.mfa_code = empty_mfa_code();
                    s.mfa_code.focused = true;
                }
                changed = true;
                t
            }
        };
        let left = CONFIRM_ARM_SECONDS - (t - since);
        if left <= 0.0 {
            s.question_armed = true;
            // The buttons of the dialog become enabled.
            changed = true;
        } else {
            // A `poll` must run when the delay ends, even if nothing else wakes the UI.
            tx.wake_after(Duration::from_secs_f64(left));
        }
    }
    if s.error.is_some() {
        let since = *s.error_since.get_or_insert(t);
        let left = TOAST_SECONDS - (t - since);
        if left <= 0.0 {
            s.clear_error();
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64(left));
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

const SCENES: &[Scene] = &[
    ("one-session", one_session),
    ("create-dialog", views::create_dialog::open),
    (
        "create-dialog-region",
        views::create_dialog::open_with_region_list,
    ),
];

/// One inactive IAM User. Needs no AWS: it only writes the workspace and the secret store.
fn one_session(s: &mut Ui) {
    let created = s.leapp.create_iam_user(NewIamUser {
        name: "My IAM User".into(),
        access_key_id: "AKIAEXAMPLE".into(),
        secret_access_key: "example-secret".into(),
        profile_name: "default".into(),
        region: "us-east-1".into(),
        mfa_device: None,
    });
    if let Err(e) = created {
        s.show_error(e.to_string());
    }
    s.refresh();
}

/// Runs when the window closes, and on Cmd+Q and the Dock quit (Rune calls `on_close` for all
/// three). Cleans up the managed profiles, then quits. The UI thread waits for the cleanup: it is
/// local file and keychain work.
pub fn on_close(s: &mut Ui, tx: &mut Tx) {
    // Drop the open questions, and the ones still in the bridge: their tasks get no (overwrite)
    // or cancel (MFA) and end, so no start waits for a dialog that is gone. A `Refreshed` or
    // `Failed` message in the bridge is lost too: the app quits now.
    drop(s.bridge.drain());
    s.questions.clear();
    s.question_since = None;
    s.question_armed = false;
    s.mfa_code = empty_mfa_code();
    let leapp = s.leapp.clone();
    if let Err(e) = s.rt.block_on(async move { leapp.shutdown().await }) {
        eprintln!("leapp-app: shutdown failed: {e}");
    }
    tx.quit();
}

/// The window can not get shorter than this: the toolbar, the header and a few rows.
const MIN_WINDOW_HEIGHT: f32 = 480.0;

pub fn spec() -> AppSpec<Ui> {
    AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        scenes: SCENES,
        min_size: Some((views::main_window::MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)),
        ..AppSpec::new("Leapp RS", (1100.0, 700.0), init, view)
    }
}
