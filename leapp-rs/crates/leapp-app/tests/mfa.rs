//! The MFA code dialog and the question queue.

mod common;

use std::time::{Duration, Instant};

use common::{AppHarness, MFA_DEVICE, iam_user, mfa_iam_user};
use leapp_core::{SessionId, SessionStatus};
use rune::Key;

fn status(ui: &leapp_app::Ui, id: SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

/// The params of the last `GetSessionToken` call that the fake received.
fn last_token_code(h: &AppHarness) -> Option<String> {
    let requests = h.fake.requests();
    let call = requests.iter().rfind(|r| r.action == "GetSessionToken")?;
    assert_eq!(
        call.params.get("SerialNumber").map(String::as_str),
        Some(MFA_DEVICE)
    );
    call.params.get("TokenCode").cloned()
}

/// A harness with one IAM User `dev` that has an MFA device, and its MFA dialog open and armed.
fn with_mfa_dialog() -> (AppHarness, SessionId) {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(mfa_iam_user("dev", "default")).unwrap());
    let (x, y) = h.rt.find(&format!("row-{id}")).unwrap();
    for _ in 0..2 {
        h.rt.pointer_down(x - 100.0, y);
        h.rt.pointer_up(x - 100.0, y);
        h.frames(1);
    }
    h.settle(|ui| ui.pending_mfa().is_some());
    assert_eq!(h.rt.state.pending_mfa(), Some("dev"));
    assert!(h.has("dlg-mfa"));
    h.wait_for_arming();
    (h, id)
}

/// Focuses the code field and deletes its text.
fn clear_code(h: &mut AppHarness) {
    h.tap("fld-mfa-code");
    h.press(Key::End);
    for _ in 0..10 {
        h.press(Key::Backspace);
    }
}

#[test]
fn the_mfa_dialog_sends_the_code() {
    let (mut h, id) = with_mfa_dialog();
    assert!(h.has_text("MFA code for dev"), "the dialog title");

    h.type_text("123456");
    assert!(
        !h.tree_contains("123456"),
        "the code is not in the element tree"
    );
    h.tap("btn-mfa-submit");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);

    assert!(!h.has("dlg-mfa"));
    assert_eq!(h.rt.state.pending_question_count(), 0);
    assert_eq!(last_token_code(&h).as_deref(), Some("123456"));
}

#[test]
fn enter_submits_the_code() {
    let (mut h, id) = with_mfa_dialog();
    h.type_text("654321");
    h.press(Key::Enter);
    h.settle(|ui| status(ui, id) == SessionStatus::Active);

    assert!(!h.has("dlg-mfa"));
    assert_eq!(last_token_code(&h).as_deref(), Some("654321"));
}

#[test]
fn a_short_or_spaced_code_cannot_be_submitted() {
    let (mut h, id) = with_mfa_dialog();

    for bad in ["12345", "1 2345", "1234567", "12345a"] {
        h.type_text(bad);
        assert!(h.is_disabled("btn-mfa-submit"), "{bad:?} disables Submit");
        h.tap("btn-mfa-submit");
        h.press(Key::Enter);
        h.frames(5);
        assert!(h.has("dlg-mfa"), "{bad:?} keeps the dialog open");
        assert_eq!(h.fake.count("GetSessionToken"), 0, "{bad:?} sends nothing");
        assert_eq!(status(&h.rt.state, id), SessionStatus::Pending);
        clear_code(&mut h);
    }

    h.type_text(" 123456 ");
    assert!(!h.is_disabled("btn-mfa-submit"));
    h.tap("btn-mfa-submit");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    assert_eq!(last_token_code(&h).as_deref(), Some("123456"));
}

#[test]
fn cancel_shows_the_mfa_error() {
    let (mut h, id) = with_mfa_dialog();
    h.type_text("123456");
    h.tap("btn-mfa-cancel");
    h.settle_with(|h| h.has("toast-error"));

    assert!(!h.has("dlg-mfa"));
    assert_eq!(
        h.text_of("toast-error").as_deref(),
        Some("Missing Multi Factor Authentication code")
    );
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("inactive")
    );
    assert_eq!(h.fake.count("GetSessionToken"), 0);
}

#[test]
fn the_mfa_dialog_ignores_answers_until_armed() {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(mfa_iam_user("dev", "default")).unwrap());
    h.tap(&format!("status-{id}"));
    h.settle(|ui| ui.pending_mfa().is_some());
    h.type_text("123456");

    assert!(h.is_disabled("btn-mfa-submit"));
    assert!(h.is_disabled("btn-mfa-cancel"));
    h.tap("btn-mfa-cancel");
    h.tap("btn-mfa-submit");
    assert_eq!(h.rt.state.pending_mfa(), Some("dev"), "not answered yet");

    h.wait_for_arming();
    assert!(!h.is_disabled("btn-mfa-submit"));
    h.tap("btn-mfa-submit");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

#[test]
fn the_overwrite_buttons_look_disabled_until_armed() {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap());
    let dir = h.home.path().join(".aws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("credentials"),
        "[default]\naws_access_key_id = AKIAUSER\n",
    )
    .unwrap();
    h.tap(&format!("status-{id}"));
    h.settle_with(|h| h.has("dlg-overwrite"));

    assert!(h.is_disabled("btn-overwrite-yes"));
    assert!(h.is_disabled("btn-overwrite-no"));

    h.wait_for_arming();
    assert!(!h.is_disabled("btn-overwrite-yes"));
    assert!(!h.is_disabled("btn-overwrite-no"));
}

#[test]
fn an_mfa_question_waits_behind_an_overwrite_question() {
    let (mut h, (plain, mfa)) = AppHarness::with_setup(|l| {
        (
            l.create_iam_user(iam_user("one", "first")).unwrap(),
            l.create_iam_user(mfa_iam_user("two", "second")).unwrap(),
        )
    });
    let dir = h.home.path().join(".aws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("credentials"),
        "[first]\naws_access_key_id = AKIAONE\n",
    )
    .unwrap();
    h.tap(&format!("status-{plain}"));
    h.settle_with(|h| h.has("dlg-overwrite"));
    let leapp = h.leapp.clone();
    h.rt.state.run(async move { leapp.start(mfa).await });
    h.settle(|ui| ui.pending_question_count() == 2);

    assert!(h.has("dlg-overwrite"));
    assert!(!h.has("dlg-mfa"), "one question at a time");
    assert_eq!(h.rt.state.pending_mfa(), None);

    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| ui.pending_mfa() == Some("two"));
    assert!(h.has("dlg-mfa"));
    assert!(
        h.is_disabled("btn-mfa-cancel"),
        "the MFA question arms again"
    );

    h.wait_for_arming();
    h.type_text("123456");
    h.tap("btn-mfa-submit");
    h.settle(|ui| {
        status(ui, mfa) == SessionStatus::Active && status(ui, plain) == SessionStatus::Active
    });
}

#[test]
fn closing_the_window_with_the_mfa_dialog_open_quits() {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(mfa_iam_user("dev", "default")).unwrap());
    let leapp = h.leapp.clone();
    let start = h.spawn(async move { leapp.start(id).await });
    h.settle(|ui| ui.pending_mfa().is_some());

    let began = Instant::now();
    h.rt.update(leapp_app::on_close);
    assert!(began.elapsed() < Duration::from_secs(2), "on_close returns");

    let result = h
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), start).await })
        .expect("the start ends")
        .unwrap();
    assert!(result.is_err(), "the start ends with an error");
    assert!(!credentials(&h).contains("[default]"));
    assert_eq!(h.fake.count("GetSessionToken"), 0);
}
