//! Messages from background tasks to the UI thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use leapp_core::Prompter;
use rune::Waker;
use tokio::sync::oneshot;

/// A message for the UI. `poll` reads it on the next frame.
#[derive(Debug)]
pub enum Msg {
    /// The workspace changed. Read it again.
    Refreshed,
    /// An operation failed. The text is for the user.
    Failed(String),
    /// Core asks if it can overwrite the user's named profile. Send the answer on `reply`.
    ConfirmOverwrite {
        profile: String,
        reply: oneshot::Sender<bool>,
    },
    /// The start of session `id` that the UI asked for ended. `error` is for the user.
    StartDone {
        id: leapp_core::SessionId,
        error: Option<String>,
    },
    /// The create or save of the session form ended. `poll` closes the form, or shows the error
    /// in it.
    FormDone(Result<(), leapp_core::Error>),
    /// The rename of the named profile `profile_id` that the Settings dialog asked for ended.
    RenameDone {
        profile_id: uuid::Uuid,
        result: Result<(), leapp_core::Error>,
    },
    /// The change of region or named profile that a change dialog sent with `token` ended.
    ChangeDone {
        token: u64,
        result: Result<(), leapp_core::Error>,
    },
    /// Core asks for the MFA code of the session named `session`. Send the code, or `None` to
    /// cancel, on `reply`.
    AskMfa {
        session: String,
        reply: oneshot::Sender<Option<String>>,
    },
}

/// A channel from any thread to the UI thread. Clones share the same channel and waker.
#[derive(Clone)]
pub struct Bridge {
    tx: Sender<Msg>,
    rx: Arc<Mutex<Receiver<Msg>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Bridge {
    pub fn new() -> Bridge {
        let (tx, rx) = mpsc::channel();
        Bridge {
            tx,
            rx: Arc::new(Mutex::new(rx)),
            waker: Arc::new(Mutex::new(None)),
        }
    }

    /// Queues `m` and wakes the UI thread, if a waker is set.
    pub fn send(&self, m: Msg) {
        // The receiver lives in every clone, so this send cannot fail while `self` exists.
        let _ = self.tx.send(m);
        let waker = self.waker.lock().unwrap().clone();
        if let Some(wake) = waker {
            wake();
        }
    }

    /// Sets the function that wakes the UI thread, then calls it one time. Messages sent before
    /// this call had no waker, so this wake makes `poll` read them.
    pub(crate) fn set_waker(&self, waker: Waker) {
        *self.waker.lock().unwrap() = Some(waker.clone());
        // Call it outside the lock.
        waker();
    }

    /// All queued messages, oldest first.
    pub(crate) fn drain(&self) -> Vec<Msg> {
        self.rx.lock().unwrap().try_iter().collect()
    }
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge::new()
    }
}

/// A [`Prompter`] that asks the user through the UI.
pub struct UiPrompter {
    bridge: Bridge,
}

impl UiPrompter {
    pub fn new(bridge: Bridge) -> Self {
        UiPrompter { bridge }
    }
}

#[async_trait]
impl Prompter for UiPrompter {
    /// Sends [`Msg::ConfirmOverwrite`] and waits for the answer. No answer (the UI dropped the
    /// question) means no.
    async fn confirm_profile_overwrite(&self, profile: &str) -> bool {
        let (reply, answer) = oneshot::channel();
        self.bridge.send(Msg::ConfirmOverwrite {
            profile: profile.to_string(),
            reply,
        });
        answer.await.unwrap_or(false)
    }

    /// Sends [`Msg::AskMfa`] and waits for the code. No answer (the UI dropped the question)
    /// means cancel.
    async fn ask_mfa_code(&self, session_name: &str) -> Option<String> {
        let (reply, answer) = oneshot::channel();
        self.bridge.send(Msg::AskMfa {
            session: session_name.to_string(),
            reply,
        });
        answer.await.ok().flatten()
    }
}
