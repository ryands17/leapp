//! The bridge wakes the UI thread when a message is queued.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::AppHarness;
use leapp_app::Msg;
use rune::Waker;

fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let waker: Waker = Arc::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    (waker, count)
}

#[test]
fn send_wakes_the_ui_once_per_message() {
    let mut h = AppHarness::new();
    let (waker, count) = counting_waker();
    leapp_app::setup(&mut h.rt.state, waker);
    let before = count.load(Ordering::SeqCst);

    h.bridge.send(Msg::Refreshed);

    assert_eq!(count.load(Ordering::SeqCst), before + 1);
}

#[test]
fn send_from_a_tokio_task_wakes_the_ui() {
    let mut h = AppHarness::new();
    let (waker, count) = counting_waker();
    leapp_app::setup(&mut h.rt.state, waker);
    let before = count.load(Ordering::SeqCst);

    let bridge = h.bridge.clone();
    h.block_on(async move {
        tokio::spawn(async move { bridge.send(Msg::Refreshed) })
            .await
            .unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while count.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < deadline, "no wake from the task");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(count.load(Ordering::SeqCst), before + 1);
}

#[test]
fn setup_wakes_the_ui_for_messages_queued_before_it() {
    let mut h = AppHarness::new();
    h.bridge.send(Msg::Refreshed);
    let (waker, count) = counting_waker();
    assert_eq!(count.load(Ordering::SeqCst), 0);

    leapp_app::setup(&mut h.rt.state, waker);

    assert!(count.load(Ordering::SeqCst) >= 1);
}
