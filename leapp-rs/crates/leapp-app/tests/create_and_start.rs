mod common;

use common::AppHarness;
use leapp_core::regions::REGIONS;
use rune::Key;

fn fill(h: &mut AppHarness, name: &str, key_id: &str, secret: &str) {
    for (field, value) in [
        ("fld-name", name),
        ("fld-access-key", key_id),
        ("fld-secret-key", secret),
    ] {
        h.tap(field);
        if !value.is_empty() {
            h.type_text(value);
        }
    }
}

#[test]
fn create_dialog_adds_a_session() {
    let mut h = AppHarness::new();
    assert!(!h.has("dlg-iam-user"));
    h.tap("btn-create");
    assert!(h.has("dlg-iam-user"));

    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    assert!(!h.has("dlg-iam-user"));
    let rows = h.row_keys();
    assert_eq!(rows.len(), 1);
    let id = rows[0].strip_prefix("row-").unwrap().to_string();
    assert_eq!(
        h.text_of(&format!("cell-name-{id}")).as_deref(),
        Some("dev")
    );
    assert_eq!(
        h.text_of(&format!("cell-profile-{id}")).as_deref(),
        Some("default")
    );
    assert_eq!(
        h.text_of(&format!("cell-region-{id}")).as_deref(),
        Some("US-EAST-1")
    );
    let secrets = h.secrets.snapshot();
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-secret-access-key"))
            .map(String::as_str),
        Some("SECRET-dev")
    );
    assert_eq!(
        secrets
            .get(&format!("iam-user-{id}-access-key-id"))
            .map(String::as_str),
        Some("AKID-dev")
    );
}

#[test]
fn create_is_blocked_until_required_fields_are_set() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("err-name"));

    assert!(h.has("dlg-iam-user"));
    assert!(h.leapp.workspace().sessions.is_empty());
    assert!(h.text_of("err-name").is_some());
    // Core reports the first bad field only.
    assert!(!h.has("err-access-key"));
    assert!(!h.has("err-profile"));
    assert!(!h.has("err-region"));
}

#[test]
fn a_bad_profile_name_shows_under_the_profile_field() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("sel-profile-value-trigger");
    h.type_text("[bad]");
    h.frames(10);
    h.tap("sel-profile-option-__new__");
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has("err-profile"));

    assert!(h.has("dlg-iam-user"));
    assert!(h.leapp.workspace().sessions.is_empty());
    assert!(h.text_of("err-profile").is_some());
    assert!(!h.has("err-name"));
    assert!(h.secrets.snapshot().is_empty(), "nothing is saved on error");
}

#[test]
fn the_form_starts_with_default_profile_and_region() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    assert_eq!(h.rt.state.create_form().profile(), "default");
    assert_eq!(h.rt.state.create_form().region(), "us-east-1");
    assert!(h.has("sel-region"));
}

#[test]
fn typing_in_the_region_search_shows_only_matching_regions() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("sel-region-value-trigger");
    h.type_text("eu-w");
    h.frames(10);

    let shown = h.keys_with_prefix("sel-region-option-");
    let mut want: Vec<String> = REGIONS
        .iter()
        .filter(|r| r.starts_with("eu-west-"))
        .map(|r| format!("sel-region-option-{r}"))
        .collect();
    let mut got = shown.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want, "only the eu-west regions are listed");
}

#[test]
fn a_region_found_by_search_is_used() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("sel-region-value-trigger");
    h.type_text("ap-southeast-2");
    h.frames(10);
    h.tap("sel-region-option-ap-southeast-2");
    assert_eq!(h.rt.state.create_form().region(), "ap-southeast-2");

    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    assert_eq!(h.leapp.workspace().sessions[0].region, "ap-southeast-2");
}

#[test]
fn enter_in_the_region_search_picks_the_first_match() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("sel-region-value-trigger");
    h.type_text("eu-north-1");
    h.frames(10);
    h.press(Key::Enter);

    assert_eq!(h.rt.state.create_form().region(), "eu-north-1");
    assert!(
        h.keys_with_prefix("sel-region-option-").is_empty(),
        "the list closes after a choice"
    );
}

#[test]
fn cancel_closes_the_dialog_and_wipes_the_fields() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-cancel");

    assert!(!h.has("dlg-iam-user"));
    assert!(h.leapp.workspace().sessions.is_empty());
    h.tap("btn-create");
    let form = h.rt.state.create_form();
    assert!(
        form.is_blank(),
        "the fields are empty when the dialog opens again"
    );
    assert_eq!(form.profile(), "default");
}

#[test]
fn the_secret_never_shows_in_the_element_tree() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.frames(2);

    for e in h.rt.elements() {
        for t in [&e.text, &e.label].into_iter().flatten() {
            assert!(!t.contains("SECRET-dev"), "secret in {:?}: {t}", e.key);
        }
    }
    assert!(!format!("{:?}", h.rt.state.create_form()).contains("SECRET-dev"));
}

#[test]
fn success_wipes_the_fields() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    h.tap("btn-create");
    assert!(h.rt.state.create_form().is_blank());
    assert!(!h.has("err-name"));
}

/// Taps Create Session and waits until the core call ends: `key` shows.
fn submit_until(h: &mut AppHarness, key: &str) {
    h.tap("btn-create-submit");
    h.settle_with(|h| h.has(key));
}

#[test]
fn each_input_error_shows_under_its_field() {
    // No access key.
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "", "SECRET-dev");
    submit_until(&mut h, "err-access-key");
    assert!(!h.has("err-name") && !h.has("err-secret-key") && !h.has("err-form"));

    // No secret key.
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "");
    submit_until(&mut h, "err-secret-key");
    assert!(!h.has("err-access-key") && !h.has("err-form"));

    // The secret store fails: the error is not about one field.
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.store.fail_next_set();
    submit_until(&mut h, "err-form");
    assert!(h.text_of("err-form").unwrap().contains("refused the write"));
    assert!(h.has("dlg-iam-user"), "the dialog stays open");
    assert!(h.leapp.workspace().sessions.is_empty());
    assert!(!h.has("err-access-key") && !h.has("err-secret-key"));
}

#[test]
fn typing_in_a_field_clears_its_error() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    submit_until(&mut h, "err-name");

    // An edit of another field does not clear the error of the name.
    h.tap("fld-access-key");
    h.type_text("A");
    assert!(h.has("err-name"), "the error of the name stays");

    h.tap("fld-name");
    h.type_text("d");
    assert!(!h.has("err-name"), "typing in the name clears its error");

    // The same for a second field: its error goes, the others stay.
    submit_until(&mut h, "err-secret-key");
    h.tap("sel-profile-value-trigger");
    h.type_text("x");
    h.frames(10);
    h.tap("sel-profile-option-__new__");
    assert!(
        h.has("err-secret-key"),
        "an edit of the profile keeps the secret key error"
    );
    h.tap("fld-secret-key");
    h.type_text("s");
    assert!(!h.has("err-secret-key"));
}

/// The caret x offset inside the text input `key`. Plain glyphs and bullets have different
/// widths, so it shows if the field draws the secret or masks it.
fn caret_x(h: &AppHarness, key: &str) -> f32 {
    let suffix = format!("{key}/value/input");
    let input =
        h.rt.elements()
            .into_iter()
            .find(|e| e.path.ends_with(&suffix))
            .unwrap_or_else(|| panic!("no text input {key:?}"));
    input.caret.expect("the field has a caret")[0] - input.rect[0]
}

#[test]
fn the_eye_shows_the_secret() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("fld-secret-key");
    h.type_text("SECRET-dev");
    h.tap("fld-secret-key");
    assert!(h.rt.has_secure_focus(), "the secret field is secure");
    assert!(!h.rt.state.create_form().secret_shown());
    assert_eq!(
        h.text_of("btn-secret-eye").as_deref(),
        Some("Show secret key")
    );

    let masked_caret = caret_x(&h, "fld-secret-key");

    h.tap("btn-secret-eye");
    assert!(
        h.rt.state.create_form().secret_shown(),
        "the field shows the secret"
    );
    assert_eq!(
        h.text_of("btn-secret-eye").as_deref(),
        Some("Hide secret key")
    );
    h.tap("fld-secret-key");
    assert!(
        h.rt.has_secure_focus(),
        "a shown secret keeps the secure protections"
    );
    assert!(
        !h.tree_contains("SECRET-dev"),
        "a shown secret is not in the element tree"
    );
    assert_ne!(
        caret_x(&h, "fld-secret-key"),
        masked_caret,
        "the field draws plain text, not bullets"
    );

    h.tap("btn-secret-eye");
    assert!(
        !h.rt.state.create_form().secret_shown(),
        "the field masks the secret again"
    );
    assert_eq!(
        h.text_of("btn-secret-eye").as_deref(),
        Some("Show secret key")
    );
    h.tap("fld-secret-key");
    assert!(h.rt.has_secure_focus());
}

#[test]
fn an_mfa_device_is_saved() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("fld-mfa-device");
    h.type_text(common::MFA_DEVICE);
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    let leapp_core::SessionKind::IamUser { mfa_device, .. } = &h.leapp.workspace().sessions[0].kind
    else {
        panic!("not an IAM User");
    };
    assert_eq!(mfa_device.as_deref(), Some(common::MFA_DEVICE));
}

#[test]
fn the_dialog_fits_the_smallest_window() {
    let mut h = AppHarness::new();
    let (min_w, min_h) = leapp_app::spec()
        .min_size
        .expect("the window has a minimum size");
    h.rt.set_viewport((min_w, min_h), 1.0);
    h.frames(2);
    h.tap("btn-create");
    h.frames(30);

    for key in ["btn-create-submit", "btn-create-cancel", "fld-mfa-device"] {
        let [x, y, w, hgt] = h.rect(key);
        assert!(
            x >= 0.0 && y >= 0.0 && x + w <= min_w && y + hgt <= min_h,
            "{key} {:?} is outside the {min_w} x {min_h} window",
            [x, y, w, hgt]
        );
    }
}

#[test]
fn submit_is_disabled_while_the_session_is_saved() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    assert!(!h.is_disabled("btn-create-submit"));
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    let gate = h.store.hold_sets();
    h.tap("btn-create-submit");
    h.frames(5);

    // The core call waits in the secret store, off the render thread: the UI still draws.
    assert!(h.has("dlg-iam-user"));
    assert!(h.is_disabled("btn-create-submit"));
    h.tap("btn-create-submit");
    gate.open();
    h.settle(|ui| ui.workspace().sessions.len() == 1);
    assert!(!h.has("dlg-iam-user"));
    h.frames(5);
    assert_eq!(
        h.leapp.workspace().sessions.len(),
        1,
        "one tap, one session"
    );
}

// ---- start and stop ----

use common::iam_user;
use leapp_core::SessionStatus;

const USER_SECTION: &str = "[default]\naws_access_key_id = AKIAUSER\n";

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

fn write_credentials(h: &AppHarness, text: &str) {
    let dir = h.home.path().join(".aws");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("credentials"), text).unwrap();
}

fn status(ui: &leapp_app::Ui, id: leapp_core::SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

/// A harness with one IAM User on profile `default`.
fn with_session() -> (AppHarness, leapp_core::SessionId) {
    let (h, id) =
        AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap());
    (h, id)
}

#[test]
fn start_and_stop_from_the_table() {
    let (mut h, id) = with_session();
    let key = format!("status-{id}");
    assert_eq!(h.text_of(&key).as_deref(), Some("inactive"));

    h.fake
        .delay_next("GetSessionToken", std::time::Duration::from_millis(300));
    h.tap(&key);
    assert_eq!(
        h.text_of(&key).as_deref(),
        Some("pending"),
        "the UI shows pending at once"
    );
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    assert_eq!(h.text_of(&key).as_deref(), Some("active"));
    assert!(credentials(&h).contains("[default]"));
    assert!(credentials(&h).contains("ASIAFAKE0001"));

    h.tap(&key);
    h.settle(|ui| status(ui, id) == SessionStatus::Inactive);
    assert!(!credentials(&h).contains("[default]"));
}

#[test]
fn double_click_on_the_row_starts_the_session() {
    let (mut h, id) = with_session();
    let row = format!("row-{id}");
    let (x, y) = h.rt.find(&row).unwrap();
    for _ in 0..2 {
        h.rt.pointer_down(x - 100.0, y);
        h.rt.pointer_up(x - 100.0, y);
        h.frames(1);
    }
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    assert!(credentials(&h).contains("ASIAFAKE0001"));
}

#[test]
fn overwrite_prompt_yes_replaces_and_stop_restores() {
    let (mut h, id) = with_session();
    write_credentials(&h, USER_SECTION);
    let key = format!("status-{id}");
    h.tap(&key);
    h.settle_with(|h| h.has("dlg-overwrite"));
    assert!(h.text_of("dlg-overwrite").unwrap().contains("default"));

    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    assert!(!h.has("dlg-overwrite"));
    assert!(credentials(&h).contains("ASIAFAKE0001"));

    h.tap(&key);
    h.settle(|ui| status(ui, id) == SessionStatus::Inactive);
    assert_eq!(credentials(&h), USER_SECTION);
}

#[test]
fn overwrite_prompt_no_shows_an_error_and_keeps_the_file() {
    let (mut h, id) = with_session();
    write_credentials(&h, USER_SECTION);
    let key = format!("status-{id}");
    h.tap(&key);
    h.settle_with(|h| h.has("dlg-overwrite"));
    h.wait_for_arming();
    h.tap("btn-overwrite-no");
    h.settle_with(|h| h.has("toast-error"));

    assert!(!h.has("dlg-overwrite"));
    assert_eq!(h.text_of(&key).as_deref(), Some("inactive"));
    assert_eq!(credentials(&h), USER_SECTION);
}

#[test]
fn aws_error_is_shown_to_the_user() {
    let (mut h, id) = with_session();
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "InvalidClientTokenId",
        "The security token included in the request is invalid.",
    );
    let key = format!("status-{id}");
    h.tap(&key);
    h.settle_with(|h| h.has("toast-error"));

    assert!(h.text_of("toast-error").unwrap().contains("security token"));
    assert_eq!(h.text_of(&key).as_deref(), Some("inactive"));
}

#[test]
fn the_error_toast_hides_after_eight_seconds_and_on_close() {
    let (mut h, id) = with_session();
    h.fake
        .fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key");
    h.tap(&format!("status-{id}"));
    h.settle_with(|h| h.has("toast-error"));

    h.rt.frame(7.0);
    assert!(h.has("toast-error"), "still shown after 7 s");
    h.rt.frame(2.0);
    h.frames(1);
    assert!(!h.has("toast-error"), "hidden after 8 s");

    h.fake
        .fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key");
    h.tap(&format!("status-{id}"));
    h.settle_with(|h| h.has("toast-error"));
    h.tap("toast-error-close");
    assert!(!h.has("toast-error"));
}

const TWO_USER_SECTIONS: &str =
    "[first]\naws_access_key_id = AKIAONE\n\n[second]\naws_access_key_id = AKIATWO\n";

/// Two IAM Users on two user-owned profiles, `first` and `second`, and two overwrite questions in
/// line: the one for `first` is in front.
fn two_questions() -> (AppHarness, (leapp_core::SessionId, leapp_core::SessionId)) {
    let (mut h, ids) = AppHarness::with_setup(|l| {
        (
            l.create_iam_user(iam_user("one", "first")).unwrap(),
            l.create_iam_user(iam_user("two", "second")).unwrap(),
        )
    });
    write_credentials(&h, TWO_USER_SECTIONS);
    h.tap(&format!("status-{}", ids.0));
    h.settle_with(|h| h.has("dlg-overwrite"));
    assert!(h.text_of("dlg-overwrite").unwrap().contains("first"));

    // The second start runs while the first dialog is open.
    let leapp = h.leapp.clone();
    let second = ids.1;
    h.rt.state.run(async move { leapp.start(second).await });
    h.settle(|ui| ui.pending_question_count() == 2);
    (h, ids)
}

#[test]
fn a_second_overwrite_question_waits_for_the_first() {
    let (mut h, ids) = two_questions();
    assert!(
        h.text_of("dlg-overwrite").unwrap().contains("first"),
        "the first question stays in front"
    );

    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, ids.0) == SessionStatus::Active);
    h.settle_with(|h| {
        h.text_of("dlg-overwrite")
            .is_some_and(|t| t.contains("second"))
    });
    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, ids.1) == SessionStatus::Active);
    assert!(!h.has("dlg-overwrite"));
    assert!(credentials(&h).contains("ASIAFAKE"));
}

#[test]
fn a_double_click_does_not_answer_the_next_question() {
    let (mut h, ids) = two_questions();
    h.wait_for_arming();

    // A double-click: the first click answers "first", the second lands on the next dialog.
    h.tap("btn-overwrite-yes");
    h.tap("btn-overwrite-yes");

    assert_eq!(h.rt.state.pending_question_count(), 1);
    assert_eq!(h.rt.state.pending_confirm(), Some("second"));
    assert!(h.text_of("dlg-overwrite").unwrap().contains("second"));
    h.settle(|ui| status(ui, ids.0) == SessionStatus::Active);
    assert_eq!(status(&h.rt.state, ids.1), SessionStatus::Pending);
    assert!(
        credentials(&h).contains("AKIATWO"),
        "second is not replaced"
    );

    // After the delay, a click answers the second question.
    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, ids.1) == SessionStatus::Active);
    assert_eq!(h.rt.state.pending_question_count(), 0);
    assert!(!credentials(&h).contains("AKIATWO"));
}

#[test]
fn each_question_in_line_gets_its_own_answer() {
    let (mut h, ids) = two_questions();

    h.wait_for_arming();
    h.tap("btn-overwrite-no");
    h.settle_with(|h| h.has("toast-error"));
    assert_eq!(status(&h.rt.state, ids.0), SessionStatus::Inactive);
    assert_eq!(h.rt.state.pending_confirm(), Some("second"));

    h.wait_for_arming();
    h.tap("btn-overwrite-yes");
    h.settle(|ui| status(ui, ids.1) == SessionStatus::Active);

    let text = credentials(&h);
    assert!(
        text.starts_with("[first]\naws_access_key_id = AKIAONE\n\n[second]\n"),
        "{text}"
    );
    assert!(!text.contains("AKIATWO") && text.contains("ASIAFAKE0001"));
    assert_eq!(status(&h.rt.state, ids.0), SessionStatus::Inactive);
}
