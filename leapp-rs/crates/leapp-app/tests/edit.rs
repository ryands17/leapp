//! Edit of IAM User sessions in the session form.

mod common;

use common::{AppHarness, iam_user};
use leapp_core::{SessionId, SessionStatus};
use rune::Key;

const PLACEHOLDER: &str = "Leave empty to keep the current key";

fn status(ui: &leapp_app::Ui, id: SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

/// A harness with one IAM User `dev` (keys `AKID-dev`, `SECRET-dev`) on profile `default`.
fn with_session() -> (AppHarness, SessionId) {
    AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap())
}

fn open_edit(h: &mut AppHarness, id: SessionId) {
    h.rt.update(|s, _| s.open_edit(id));
    h.frames(2);
    assert!(h.has("dlg-iam-user"));
}

/// Focuses `field` and deletes its text.
fn clear(h: &mut AppHarness, field: &str) {
    h.tap(field);
    h.press(Key::End);
    for _ in 0..40 {
        h.press(Key::Backspace);
    }
}

fn secret(h: &AppHarness, key: &str) -> Option<String> {
    h.secrets.snapshot().get(key).cloned()
}

#[test]
fn edit_keeps_secret_fields_empty_and_keeps_old_keys() {
    let (mut h, id) = with_session();
    open_edit(&mut h, id);

    assert!(h.has_text("Edit IAM User"), "the title");
    assert_eq!(h.text_of("btn-create-submit").as_deref(), Some("Save"));
    let form = h.rt.state.create_form();
    assert_eq!(form.name(), "dev");
    assert_eq!(form.profile(), "default");
    assert_eq!(form.region(), "eu-west-1");
    assert!(
        form.access_key_id().is_empty(),
        "the access key field is empty"
    );
    assert!(form.secret_key_is_empty(), "the secret key field is empty");
    for key in ["fld-access-key", "fld-secret-key"] {
        assert_eq!(
            h.placeholder_of(key).as_deref(),
            Some(PLACEHOLDER),
            "{key} says how to keep the key"
        );
    }
    assert!(!h.tree_contains("AKID-dev") && !h.tree_contains("SECRET-dev"));

    clear(&mut h, "fld-name");
    h.type_text("prod");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().session(id).unwrap().name == "prod");

    assert!(!h.has("dlg-iam-user"), "success closes the dialog");
    assert_eq!(h.leapp.workspace().session(id).unwrap().name, "prod");
    assert_eq!(
        h.text_of(&format!("cell-name-{id}")).as_deref(),
        Some("prod")
    );
    assert_eq!(
        secret(&h, &format!("iam-user-{id}-access-key-id")).as_deref(),
        Some("AKID-dev")
    );
    assert_eq!(
        secret(&h, &format!("iam-user-{id}-secret-access-key")).as_deref(),
        Some("SECRET-dev")
    );
}

#[test]
fn edit_with_new_keys_uses_them() {
    let (mut h, id) = with_session();
    open_edit(&mut h, id);
    h.tap("fld-access-key");
    h.type_text("AKID-new");
    h.tap("fld-secret-key");
    h.type_text("SECRET-new");
    h.tap("btn-create-submit");
    h.settle_with(|h| !h.has("dlg-iam-user"));
    assert_eq!(
        secret(&h, &format!("iam-user-{id}-secret-access-key")).as_deref(),
        Some("SECRET-new")
    );

    h.tap(&format!("status-{id}"));
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    let requests = h.fake.requests();
    let call = requests
        .iter()
        .rfind(|r| r.action == "GetSessionToken")
        .unwrap();
    assert_eq!(call.access_key_id.as_deref(), Some("AKID-new"));
}

#[test]
fn edit_errors_show_under_the_field() {
    let (mut h, id) = with_session();
    open_edit(&mut h, id);
    clear(&mut h, "fld-name");
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("err-name"));

    assert!(h.has("dlg-iam-user"), "the dialog stays open");
    assert!(h.has_text("Edit IAM User"), "still in edit mode");
    assert!(!h.has("err-form"));
    assert_eq!(h.leapp.workspace().session(id).unwrap().name, "dev");
}

#[test]
fn an_mfa_device_can_be_added_and_removed() {
    let (mut h, id) = with_session();
    open_edit(&mut h, id);
    h.tap("fld-mfa-device");
    h.type_text(common::MFA_DEVICE);
    h.tap("btn-create-submit");
    h.settle_with(|h| !h.has("dlg-iam-user"));
    let mfa = |h: &AppHarness| {
        let leapp_core::SessionKind::IamUser { mfa_device, .. } =
            h.leapp.workspace().session(id).unwrap().kind.clone();
        mfa_device
    };
    assert_eq!(mfa(&h).as_deref(), Some(common::MFA_DEVICE));

    open_edit(&mut h, id);
    assert_eq!(h.rt.state.create_form().mfa_device(), common::MFA_DEVICE);
    clear(&mut h, "fld-mfa-device");
    h.tap("btn-create-submit");
    h.settle_with(|h| !h.has("dlg-iam-user"));
    assert_eq!(mfa(&h), None);
}

#[test]
fn an_edit_of_a_pending_session_shows_the_error_in_the_form() {
    let (mut h, id) = with_session();
    h.fake
        .delay_next("GetSessionToken", std::time::Duration::from_millis(1500));
    h.tap(&format!("status-{id}"));
    assert_eq!(status(&h.rt.state, id), SessionStatus::Pending);

    open_edit(&mut h, id);
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("err-form"));

    assert!(h.has("dlg-iam-user"), "the dialog stays open");
    assert!(!h.has("err-name"));
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

#[test]
fn the_restart_of_an_edit_can_ask_a_question() {
    let user_section = "[default]\naws_access_key_id = AKIAUSER\n";
    let (mut h, id) = with_session();
    let dir = h.home.path().join(".aws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("credentials"), user_section).unwrap();
    h.tap(&format!("status-{id}"));
    h.settle_with(|h| h.has("dlg-overwrite"));
    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);

    // A new region restarts the session: the stop puts the user's profile back, so the start
    // asks again. The render thread must stay free to show the question.
    open_edit(&mut h, id);
    h.tap("sel-region-value-trigger");
    h.type_text("eu-north-1");
    h.frames(10);
    h.press(Key::Enter);
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("dlg-overwrite"));
    assert!(h.has("dlg-iam-user"), "the form waits for the answer");
    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle_with(|h| !h.has("dlg-iam-user"));

    let session = h.leapp.workspace().session(id).unwrap().clone();
    assert_eq!(session.region, "eu-north-1");
    assert_eq!(session.status, SessionStatus::Active);
    assert!(credentials(&h).contains("ASIAFAKE"));
}

#[test]
fn a_failed_restart_keeps_the_form_open_and_shows_the_real_status() {
    let (mut h, id) = with_session();
    h.tap(&format!("status-{id}"));
    h.settle(|ui| status(ui, id) == SessionStatus::Active);

    open_edit(&mut h, id);
    h.fake
        .fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key");
    h.tap("fld-access-key");
    h.type_text("AKID-new");
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("err-form"));

    assert!(h.text_of("err-form").unwrap().contains("bad key"));
    assert!(h.has("dlg-iam-user"), "the dialog stays open");
    assert_eq!(
        status(&h.rt.state, id),
        SessionStatus::Inactive,
        "the main window shows the real status"
    );
    assert_eq!(
        secret(&h, &format!("iam-user-{id}-access-key-id")).as_deref(),
        Some("AKID-new"),
        "the edit stays saved"
    );
}
