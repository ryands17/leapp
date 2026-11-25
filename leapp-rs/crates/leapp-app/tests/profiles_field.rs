//! The Named profile select of the IAM User form, and the default region of the form.

mod common;

use common::AppHarness;
use leapp_core::Settings;
use rune::Key;

const NEW_OPTION: &str = "sel-profile-option-__new__";

/// A harness with the profiles `default` and `dev`.
fn with_dev_profile() -> AppHarness {
    AppHarness::with_setup(|l| l.create_profile("dev").unwrap()).0
}

fn fill(h: &mut AppHarness, name: &str, key_id: &str, secret: &str) {
    for (field, value) in [
        ("fld-name", name),
        ("fld-access-key", key_id),
        ("fld-secret-key", secret),
    ] {
        h.tap(field);
        h.type_text(value);
    }
}

/// True if the element `key`, or an element inside it, shows exactly `text`.
fn shows(h: &AppHarness, key: &str, text: &str) -> bool {
    let elements = h.rt.elements();
    let Some(el) = elements
        .iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
    else {
        return false;
    };
    let prefix = format!("{}/", el.path);
    elements
        .iter()
        .filter(|e| !e.exiting && (e.path == el.path || e.path.starts_with(&prefix)))
        .any(|e| e.text.as_deref() == Some(text))
}

/// Opens the profile list of the open form and types `text` into its search field.
fn search_profiles(h: &mut AppHarness, text: &str) {
    h.tap("sel-profile-value-trigger");
    h.type_text(text);
    h.frames(10);
}

#[test]
fn the_profile_field_lists_the_profiles() {
    let mut h = with_dev_profile();
    h.open_iam_user_form();
    assert!(h.has("sel-profile"));
    assert!(
        shows(&h, "sel-profile-value-trigger", "default"),
        "the select shows the default profile"
    );
    assert!(!h.has("fld-profile"), "the text field is gone");

    h.tap("sel-profile-value-trigger");
    assert!(h.has("sel-profile-option-default"));
    assert!(h.has("sel-profile-option-dev"));
    assert!(!h.has(NEW_OPTION), "an empty search offers no new profile");

    h.tap("sel-profile-option-dev");
    assert_eq!(h.rt.state.create_form().profile(), "dev");
    assert!(shows(&h, "sel-profile-value-trigger", "dev"));
}

#[test]
fn a_new_profile_name_can_be_typed() {
    let mut h = with_dev_profile();
    h.open_iam_user_form();
    search_profiles(&mut h, "stage");

    assert!(
        shows(&h, NEW_OPTION, "Create profile \"stage\""),
        "the create option names the new profile"
    );
    h.tap(NEW_OPTION);
    assert_eq!(h.rt.state.create_form().profile(), "stage");
    assert!(
        shows(&h, "sel-profile-value-trigger", "stage"),
        "the select shows the new name"
    );

    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    let ws = h.leapp.workspace();
    assert_eq!(ws.profile_name(ws.sessions[0].profile_id), Some("stage"));
}

#[test]
fn the_new_name_is_trimmed() {
    let mut h = with_dev_profile();
    h.open_iam_user_form();
    search_profiles(&mut h, "  stage ");
    h.press(Key::Enter);
    assert_eq!(h.rt.state.create_form().profile(), "stage");
}

#[test]
fn an_existing_name_offers_no_create_option() {
    let mut h = with_dev_profile();
    h.open_iam_user_form();
    search_profiles(&mut h, "dev");

    assert!(h.has("sel-profile-option-dev"));
    assert!(!h.has(NEW_OPTION));
}

#[test]
fn the_form_preselects_the_default_region() {
    let (mut h, ()) = AppHarness::with_setup(|l| {
        l.update_settings(Settings {
            default_region: "eu-west-1".to_string(),
        })
        .unwrap()
    });
    h.open_iam_user_form();

    assert_eq!(h.rt.state.create_form().region(), "eu-west-1");
    assert!(shows(&h, "sel-region-value-trigger", "eu-west-1"));
}

#[test]
fn the_edit_form_shows_and_changes_the_profile() {
    let (mut h, id) = AppHarness::with_setup(|l| {
        l.create_profile("dev").unwrap();
        l.create_iam_user(common::iam_user("app", "dev")).unwrap()
    });
    h.rt.update(|s, _| s.open_edit(id));
    h.frames(2);

    assert_eq!(h.rt.state.create_form().profile(), "dev");
    assert!(shows(&h, "sel-profile-value-trigger", "dev"));

    h.tap("sel-profile-value-trigger");
    h.tap("sel-profile-option-default");
    h.tap("btn-create-submit");
    h.settle_with(|h| !h.has("dlg-iam-user"));

    let ws = h.leapp.workspace();
    let session = ws.session(id).unwrap();
    assert_eq!(ws.profile_name(session.profile_id), Some("default"));
}
