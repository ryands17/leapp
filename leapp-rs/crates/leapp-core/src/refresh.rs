//! Refresh of old active sessions (spec 10.4, F1-R2, F1-R7).

use chrono::{DateTime, Utc};

use crate::Leapp;
use crate::error::Error;
use crate::model::{Session, SessionId, SessionStatus};

/// The interval at which the app calls `Leapp::refresh_tick` (F1-R7: core has no timer).
pub const REFRESH_TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// An `Active` session is refreshed when `now - start_time` is greater than this.
pub const SESSION_DURATION: chrono::Duration = chrono::Duration::seconds(1200);

/// The result of one `refresh_tick`.
#[derive(Debug, Default)]
pub struct RefreshReport {
    pub refreshed: Vec<SessionId>,
    pub failures: Vec<RefreshFailure>,
}

impl RefreshReport {
    pub fn is_empty(&self) -> bool {
        self.refreshed.is_empty() && self.failures.is_empty()
    }
}

/// A refresh that failed. The session is stopped.
#[derive(Debug)]
pub struct RefreshFailure {
    pub session_id: SessionId,
    pub session_name: String,
    pub error: Error,
}

/// What one refresh did.
pub(crate) enum Refresh {
    /// The session was not refreshed this tick, and nothing failed.
    Skipped,
    Done,
    /// The refresh failed and the session is stopped. The first error is the one of the refresh;
    /// a second one is from the release of the profile (for example `Error::BackupMissing`).
    Failed {
        name: String,
        errors: Vec<Error>,
    },
}

/// `Active`, and started more than `SESSION_DURATION` before `now`.
pub(crate) fn is_due(session: &Session, now: DateTime<Utc>) -> bool {
    session.status == SessionStatus::Active
        && session
            .start_time
            .is_some_and(|start| now - start > SESSION_DURATION)
}

impl Leapp {
    /// One pass of the refresh loop (F1-R7: the app calls it every `REFRESH_TICK`). Each session
    /// that is due is refreshed, one after another. A session that an edit, a stop or a start
    /// has now is skipped this tick. After `shutdown` began, it does nothing.
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn refresh_tick(&self) -> RefreshReport {
        let due: Vec<SessionId> = {
            let state = self.state.lock().unwrap();
            if state.closing {
                return RefreshReport::default();
            }
            let now = self.config.clock.now();
            state
                .ws
                .sessions
                .iter()
                .filter(|s| is_due(s, now))
                .map(|s| s.id)
                .collect()
        };
        let mut report = RefreshReport::default();
        for id in due {
            match self.refresh_session(id).await {
                Refresh::Skipped => {}
                Refresh::Done => report.refreshed.push(id),
                Refresh::Failed { name, errors } => {
                    report
                        .failures
                        .extend(errors.into_iter().map(|error| RefreshFailure {
                            session_id: id,
                            session_name: name.clone(),
                            error,
                        }));
                }
            }
        }
        report
    }
}
