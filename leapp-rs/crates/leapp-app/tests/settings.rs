//! The Settings dialog: the default region (General tab) and the named profiles (Profiles tab).

mod common;

use common::{AppHarness, iam_user};
use leapp_core::{SessionId, Workspace};
use rune::{Key, KeyEvent, Mods};
use uuid::Uuid;

/// The ID of the profile `name` in `ws`.
fn profile_id(ws: &Workspace, name: &str) -> Uuid {
    ws.profiles
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no profile {name:?}"))
        .id
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

/// Selects all the text of the focused field (Cmd+A).
fn select_all(h: &mut AppHarness) {
    h.rt.key(KeyEvent {
        key: Key::Command('a'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    h.frames(1);
}

/// Opens the Settings dialog on its Profiles tab.
fn open_profiles(h: &mut AppHarness) {
    h.tap("btn-settings");
    assert!(h.has("dlg-settings"), "the settings dialog is open");
    h.tap("tab-profiles");
}

/// Types `name` in the new profile field and taps the add button.
fn add_profile(h: &mut AppHarness, name: &str) {
    h.tap("fld-new-profile");
    h.type_text(name);
    h.tap("btn-profile-add");
}

/// A harness with the profile `dev` and the IAM User session `app` on it.
fn with_session_on_dev() -> (AppHarness, SessionId) {
    AppHarness::with_setup(|l| {
        l.create_profile("dev").unwrap();
        l.create_iam_user(iam_user("app", "dev")).unwrap()
    })
}

#[test]
fn the_default_region_is_set_in_settings() {
    let mut h = AppHarness::new();
    h.tap("btn-settings");
    h.tap("tab-general");
    assert!(
        shows(&h, "sel-default-region-value-trigger", "us-east-1"),
        "the select shows the saved default region"
    );

    h.tap("sel-default-region-value-trigger");
    h.type_text("eu-west-1");
    h.frames(10);
    h.tap("sel-default-region-option-eu-west-1");

    assert_eq!(h.leapp.workspace().settings.default_region, "eu-west-1");
    assert!(shows(&h, "sel-default-region-value-trigger", "eu-west-1"));

    h.tap("btn-settings-close");
    assert!(!h.has("dlg-settings"), "the dialog is closed");
    h.open_iam_user_form();
    assert_eq!(h.rt.state.create_form().region(), "eu-west-1");
    assert!(shows(&h, "sel-region-value-trigger", "eu-west-1"));
}

#[test]
fn a_profile_is_added() {
    let mut h = AppHarness::new();
    open_profiles(&mut h);
    add_profile(&mut h, "dev");

    let id = profile_id(&h.leapp.workspace(), "dev");
    assert_eq!(
        h.text_of(&format!("profile-row-{id}")).as_deref(),
        Some("dev")
    );
    assert!(!h.has("err-new-profile"));

    add_profile(&mut h, "default");
    assert_eq!(
        h.text_of("err-new-profile").as_deref(),
        Some("\"default\" is not a valid profile name")
    );
    assert_eq!(h.leapp.workspace().profiles.len(), 2, "no profile is added");
}

#[test]
fn a_profile_is_renamed() {
    let (mut h, sid) = with_session_on_dev();
    let id = profile_id(&h.leapp.workspace(), "dev");
    open_profiles(&mut h);

    h.tap(&format!("btn-profile-rename-{id}"));
    let field = format!("fld-profile-rename-{id}");
    assert!(h.has(&field), "the rename field is open");
    // Type over the current name.
    h.tap(&field);
    select_all(&mut h);
    h.type_text("stage");
    h.tap(&format!("btn-profile-rename-save-{id}"));
    h.settle_with(|h| h.has(&format!("profile-name-{id}")));

    assert_eq!(h.leapp.workspace().profile_name(id), Some("stage"));
    assert_eq!(
        h.text_of(&format!("profile-name-{id}")).as_deref(),
        Some("stage")
    );
    assert_eq!(
        h.text_of(&format!("cell-profile-{sid}")).as_deref(),
        Some("stage")
    );
}

#[test]
fn a_rename_to_a_name_in_use_shows_the_error_under_the_field() {
    let (mut h, _) = AppHarness::with_setup(|l| {
        l.create_profile("dev").unwrap();
        l.create_profile("stage").unwrap();
    });
    let id = profile_id(&h.leapp.workspace(), "dev");
    open_profiles(&mut h);

    h.tap(&format!("btn-profile-rename-{id}"));
    let field = format!("fld-profile-rename-{id}");
    h.tap(&field);
    select_all(&mut h);
    h.type_text("stage");
    h.tap(&format!("btn-profile-rename-save-{id}"));
    let err = format!("err-profile-rename-{id}");
    h.settle_with(|h| h.has(&err));

    assert_eq!(h.text_of(&err).as_deref(), Some("Profile already exists"));
    assert!(h.has(&field), "the field stays open");
    assert_eq!(h.leapp.workspace().profile_name(id), Some("dev"));
}

#[test]
fn delete_asks_and_moves_the_sessions() {
    let (mut h, sid) = with_session_on_dev();
    let id = profile_id(&h.leapp.workspace(), "dev");
    open_profiles(&mut h);

    h.tap(&format!("btn-profile-delete-{id}"));
    assert_eq!(
        h.text_of("dlg-delete-profile").as_deref(),
        Some("Delete profile dev? 1 session that uses it will move to the default profile.")
    );
    assert!(h.is_disabled("btn-delete-profile-yes"));
    h.tap("btn-delete-profile-yes");
    assert!(
        h.has("dlg-delete-profile"),
        "an answer before arming is ignored"
    );

    h.wait_for_arming();
    assert!(!h.is_disabled("btn-delete-profile-yes"));
    h.tap("btn-delete-profile-yes");
    let row = format!("profile-row-{id}");
    h.settle_with(|h| !h.has(&row));

    assert!(!h.has("dlg-delete-profile"));
    assert!(h.leapp.workspace().profile_name(id).is_none());
    assert_eq!(
        h.text_of(&format!("cell-profile-{sid}")).as_deref(),
        Some("default")
    );
}

#[test]
fn delete_of_an_unused_profile_asks_without_the_second_sentence() {
    let (mut h, ()) = AppHarness::with_setup(|l| {
        l.create_profile("dev").unwrap();
    });
    let id = profile_id(&h.leapp.workspace(), "dev");
    open_profiles(&mut h);

    h.tap(&format!("btn-profile-delete-{id}"));
    assert_eq!(
        h.text_of("dlg-delete-profile").as_deref(),
        Some("Delete profile dev?")
    );
    h.wait_for_arming();
    h.tap("btn-delete-profile-no");
    assert!(!h.has("dlg-delete-profile"));
    assert!(h.has(&format!("profile-row-{id}")), "no answers no delete");
    assert_eq!(h.leapp.workspace().profile_name(id), Some("dev"));
}

#[test]
fn the_default_profile_has_no_rename_or_delete() {
    let (mut h, ()) = AppHarness::with_setup(|l| {
        l.create_profile("dev").unwrap();
    });
    let ws = h.leapp.workspace();
    let default = profile_id(&ws, "default");
    let dev = profile_id(&ws, "dev");
    open_profiles(&mut h);

    assert!(h.has(&format!("profile-row-{default}")));
    assert!(!h.has(&format!("btn-profile-rename-{default}")));
    assert!(!h.has(&format!("btn-profile-delete-{default}")));
    assert!(h.has(&format!("btn-profile-rename-{dev}")));
    assert!(h.has(&format!("btn-profile-delete-{dev}")));
}
