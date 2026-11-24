//! The background refresh loop of the app (spec 10.4, F1-R7).

mod common;

use std::time::Duration;

use common::{AppHarness, iam_user, mfa_iam_user};
use leapp_core::{SecretStore, SessionId, SessionStatus};

/// A token of the fake lives 36000 s. One second more and it is expired.
const PAST_THE_TOKEN: jiff::SignedDuration = jiff::SignedDuration::from_secs(36001);

fn status(ui: &leapp_app::Ui, id: SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

fn write_credentials(h: &AppHarness, text: &str) {
    let dir = h.home.path().join(".aws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("credentials"), text).unwrap();
}

/// Starts session `id` from the table and waits until it is active.
fn start(h: &mut AppHarness, id: SessionId) {
    h.tap(&format!("status-{id}"));
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

/// A harness with one IAM User `dev` on profile `default`, started from the table.
fn with_active_session() -> (AppHarness, SessionId) {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap());
    start(&mut h, id);
    assert!(credentials(&h).contains("fake-token-1"));
    (h, id)
}

#[test]
fn the_loop_writes_new_credentials_after_the_token_expires() {
    let (mut h, id) = with_active_session();
    h.clock.advance(PAST_THE_TOKEN);

    let _refresh = h.spawn_refresh_loop();
    h.settle_with(|h| credentials(h).contains("fake-token-2"));

    assert!(!credentials(&h).contains("fake-token-1"));
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("active")
    );
    assert!(!h.has("toast-error"));
}

#[test]
fn a_refresh_failure_shows_in_the_toast() {
    let (mut h, id) = with_active_session();
    h.clock.advance(PAST_THE_TOKEN);
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "ExpiredToken",
        "The security token expired",
    );

    let _refresh = h.spawn_refresh_loop();
    h.settle_with(|h| h.has("toast-error"));

    assert_eq!(
        h.text_of("toast-error").as_deref(),
        Some("Refresh of dev failed: The security token expired")
    );
    h.settle(|ui| status(ui, id) == SessionStatus::Inactive);
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("inactive")
    );
    assert!(!credentials(&h).contains("[default]"));
}

#[test]
fn two_failures_of_one_session_show_in_one_message() {
    const USER_SECTION: &str = "[default]\naws_access_key_id = AKIAUSER\n";
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap());
    write_credentials(&h, USER_SECTION);
    h.tap(&format!("status-{id}"));
    h.settle_with(|h| h.has("dlg-overwrite"));
    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    // The backup of the user's section is gone, so the release after the failed refresh fails
    // too.
    h.secrets.delete("aws-profile-backup-default").unwrap();
    h.clock.advance(PAST_THE_TOKEN);
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "ExpiredToken",
        "The security token expired",
    );

    let _refresh = h.spawn_refresh_loop();
    h.settle_with(|h| h.has("toast-error"));
    // More frames: a second message would replace the first one in the toast.
    h.frames(10);

    let missing = leapp_core::Error::BackupMissing("default".into()).to_string();
    assert_eq!(
        h.text_of("toast-error"),
        Some(format!(
            "Refresh of dev failed: The security token expired; {missing}"
        ))
    );
}

#[test]
fn the_loop_runs_again_after_a_tick_panics() {
    let (mut h, (a, b)) = AppHarness::with_setup(|l| {
        (
            l.create_iam_user(iam_user("first", "a")).unwrap(),
            l.create_iam_user(iam_user("second", "b")).unwrap(),
        )
    });
    start(&mut h, a);
    start(&mut h, b);
    assert!(credentials(&h).contains("fake-token-2"));
    // The tick refreshes `first` before `second`, and panics in the refresh of `first`.
    h.store
        .panic_on_next_get(&format!("iam-user-{a}-access-key-id"));
    h.clock.advance(PAST_THE_TOKEN);

    let _refresh = h.spawn_refresh_loop();

    // A later tick refreshes `second`.
    h.settle_with(|h| credentials(h).contains("fake-token-3"));
    assert_eq!(h.text_of(&format!("status-{b}")).as_deref(), Some("active"));

    // The panic did not leave `first` `Pending`: it is `Inactive` and starts again.
    h.settle(|ui| status(ui, a) == SessionStatus::Inactive);
    assert_eq!(
        h.text_of(&format!("status-{a}")).as_deref(),
        Some("inactive")
    );
    start(&mut h, a);
    assert_eq!(h.text_of(&format!("status-{a}")).as_deref(), Some("active"));
}

#[test]
fn closing_the_window_stops_the_loop() {
    let (mut h, _) = with_active_session();
    let refresh = h.spawn_refresh_loop();
    h.rt.state.set_refresh_loop(refresh.abort_handle());

    h.rt.update(leapp_app::on_close);

    let ended = h
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), refresh).await })
        .expect("the loop ends after on_close");
    assert!(ended.expect_err("the loop is stopped").is_cancelled());
}

#[test]
fn a_refresh_mfa_question_shows_over_the_create_dialog() {
    let (mut h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(mfa_iam_user("dev", "default")).unwrap());
    h.tap(&format!("status-{id}"));
    h.settle(|ui| ui.pending_mfa().is_some());
    h.wait_for_arming();
    h.type_text("123456");
    h.tap("btn-mfa-submit");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);

    h.tap("btn-create");
    h.tap("fld-name");
    h.type_text("half");
    assert_eq!(h.rt.state.create_form().name(), "half");

    h.clock.advance(PAST_THE_TOKEN);
    let _refresh = h.spawn_refresh_loop();
    h.settle(|ui| ui.pending_mfa().is_some());
    assert!(h.has("dlg-mfa"));
    assert!(h.has("dlg-iam-user"), "the Create dialog stays open");
    h.wait_for_arming();

    // The keys go to the MFA field, not to the Create field that had the focus.
    h.type_text("654321");
    assert_eq!(h.rt.state.create_form().name(), "half");
    assert!(!h.is_disabled("btn-mfa-submit"));
    // The tap reaches the MFA dialog: it is on top.
    h.tap("btn-mfa-submit");
    h.settle_with(|h| credentials(h).contains("fake-token-2"));

    let requests = h.fake.requests();
    let last = requests
        .iter()
        .rfind(|r| r.action == "GetSessionToken")
        .unwrap();
    assert_eq!(
        last.params.get("TokenCode").map(String::as_str),
        Some("654321")
    );
    assert!(!h.has("dlg-mfa"));
    assert!(h.has("dlg-iam-user"));
    assert_eq!(h.rt.state.create_form().name(), "half");
}
