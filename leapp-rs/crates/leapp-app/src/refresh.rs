//! The background refresh loop (spec 10.4, F1-R7: core has no timer, so the app calls
//! `Leapp::refresh_tick` every `REFRESH_TICK`).

use std::sync::Arc;
use std::time::Duration;

use leapp_core::{Leapp, RefreshReport};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::{Bridge, Msg};

/// Calls `leapp.refresh_tick()` on `rt` every `every`, the first time at once. Each failed
/// session gives one [`Msg::Failed`]; a report that is not empty also gives [`Msg::Refreshed`].
///
/// Each tick runs in its own task: a tick that panics ends only that tick, and the next tick
/// runs. Abort the returned handle to stop the loop (`on_close` does it). A tick in flight then
/// runs to its end, under the `closing` rules of core.
pub fn spawn_refresh_loop(
    leapp: Arc<Leapp>,
    rt: &tokio::runtime::Handle,
    bridge: Bridge,
    every: Duration,
) -> JoinHandle<()> {
    let ticks = rt.clone();
    rt.spawn(async move {
        let mut interval = tokio::time::interval(every);
        // A tick can wait long for an MFA code. Do not run the missed ticks in a burst after it.
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let leapp = leapp.clone();
            match ticks.spawn(async move { leapp.refresh_tick().await }).await {
                Ok(report) => send_report(&bridge, &report),
                // The panic text is not printed: it could hold any value.
                Err(e) if e.is_panic() => {
                    eprintln!("leapp-app: a refresh tick panicked; the next tick runs");
                }
                // The runtime shuts down.
                Err(_) => return,
            }
        }
    })
}

fn send_report(bridge: &Bridge, report: &RefreshReport) {
    for message in failure_messages(report) {
        bridge.send(Msg::Failed(message));
    }
    if !report.is_empty() {
        bridge.send(Msg::Refreshed);
    }
}

/// One message per failed session, in report order. The toast shows one message at a time, so
/// two errors of one session (the refresh, then the release of its profile) go in one message.
fn failure_messages(report: &RefreshReport) -> Vec<String> {
    let mut sessions: Vec<(leapp_core::SessionId, &str, Vec<String>)> = Vec::new();
    for f in &report.failures {
        let error = f.error.to_string();
        match sessions.iter_mut().find(|(id, ..)| *id == f.session_id) {
            Some((.., errors)) => errors.push(error),
            None => sessions.push((f.session_id, &f.session_name, vec![error])),
        }
    }
    sessions
        .into_iter()
        .map(|(_, name, errors)| format!("Refresh of {name} failed: {}", errors.join("; ")))
        .collect()
}
