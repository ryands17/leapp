//! Change region and change named profile from the row menu.

mod common;

use common::{AppHarness, iam_user};
use leapp_core::{NewIamUser, SessionId, SessionStatus};

const NEW_PROFILE_OPTION: &str = "sel-change-profile-option-__new__";

fn status(ui: &leapp_app::Ui, id: SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

/// A harness with one IAM User `dev` on profile `default`, in `us-east-1`.
fn with_session() -> (AppHarness, SessionId) {
    AppHarness::with_setup(|l| {
        l.create_iam_user(NewIamUser {
            region: "us-east-1".into(),
            ..iam_user("dev", "default")
        })
        .unwrap()
    })
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

/// Opens the row menu of `id` and taps the item `label`.
fn menu(h: &mut AppHarness, id: SessionId, label: &str) {
    h.tap(&format!("row-menu-{id}"));
    h.tap_menu_item(id, label);
}

fn start(h: &mut AppHarness, id: SessionId) {
    menu(h, id, "Start session");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

/// Opens the region list of the open Change region dialog and chooses `region`.
fn choose_region(h: &mut AppHarness, region: &str) {
    h.tap("sel-change-region-value-trigger");
    h.type_text(region);
    h.frames(10);
    h.tap(&format!("sel-change-region-option-{region}"));
}

#[test]
fn change_region_from_the_menu() {
    let (mut h, id) = with_session();
    start(&mut h, id);

    menu(&mut h, id, "Change region…");
    assert!(h.has("dlg-change-region"));
    assert!(
        shows(&h, "sel-change-region-value-trigger", "us-east-1"),
        "the dialog opens with the session's region"
    );
    choose_region(&mut h, "eu-west-1");
    h.tap("btn-change-region-save");
    h.settle_with(|h| !h.has("dlg-change-region"));

    assert_eq!(
        h.text_of(&format!("cell-region-{id}")).as_deref(),
        Some("EU-WEST-1")
    );
    assert!(credentials(&h).contains("region = eu-west-1"));
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("active"),
        "the session starts again"
    );
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "eu-west-1");
}

#[test]
fn change_profile_from_the_menu() {
    let (mut h, id) = with_session();
    start(&mut h, id);
    assert!(credentials(&h).contains("[default]"));

    menu(&mut h, id, "Change named profile…");
    assert!(h.has("dlg-change-profile"));
    assert!(
        shows(&h, "sel-change-profile-value-trigger", "default"),
        "the dialog opens with the session's profile"
    );
    h.tap("sel-change-profile-value-trigger");
    h.type_text("ci");
    h.frames(10);
    h.tap(NEW_PROFILE_OPTION);
    h.tap("btn-change-profile-save");
    h.settle_with(|h| !h.has("dlg-change-profile"));

    assert_eq!(
        h.text_of(&format!("cell-profile-{id}")).as_deref(),
        Some("ci")
    );
    let file = credentials(&h);
    assert!(file.contains("[ci]"), "the session writes its new profile");
    assert!(!file.contains("[default]"), "the old profile is removed");
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("active")
    );
}

#[test]
fn cancel_changes_nothing() {
    let (mut h, id) = with_session();

    menu(&mut h, id, "Change region…");
    choose_region(&mut h, "eu-west-1");
    h.tap("btn-change-region-cancel");
    h.frames(5);

    assert!(!h.has("dlg-change-region"));
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "us-east-1");
    assert_eq!(
        h.text_of(&format!("cell-region-{id}")).as_deref(),
        Some("US-EAST-1")
    );

    menu(&mut h, id, "Change named profile…");
    h.tap("sel-change-profile-value-trigger");
    h.type_text("ci");
    h.frames(10);
    h.tap(NEW_PROFILE_OPTION);
    h.tap("btn-change-profile-cancel");
    h.frames(5);

    assert!(!h.has("dlg-change-profile"));
    let ws = h.leapp.workspace();
    assert_eq!(
        ws.profile_name(ws.session(id).unwrap().profile_id),
        Some("default")
    );
    assert!(
        !ws.profiles.iter().any(|p| p.name == "ci"),
        "no profile is created"
    );
}

#[test]
fn the_menu_has_the_new_order() {
    let (mut h, id) = with_session();
    h.tap(&format!("row-menu-{id}"));
    let items: Vec<_> = h
        .keys_with_prefix(&format!("menu-{id}-item"))
        .iter()
        .map(|k| h.text_of(k).unwrap_or_default())
        .collect();
    assert_eq!(
        items,
        [
            "Start session",
            "Change region…",
            "Change named profile…",
            "Copy account number",
            "Edit session",
            "Delete session"
        ]
    );
}

#[test]
fn save_and_cancel_are_locked_while_the_change_runs() {
    let (mut h, id) = with_session();
    menu(&mut h, id, "Change region…");
    choose_region(&mut h, "eu-west-1");

    let gate = h.hold_workers();
    h.tap("btn-change-region-save");
    h.frames(3);
    assert!(h.is_disabled("btn-change-region-save"));
    assert!(h.is_disabled("btn-change-region-cancel"));
    // A second save while the first runs sends nothing more.
    h.tap("btn-change-region-save");
    h.tap("btn-change-region-cancel");
    assert!(
        h.has("dlg-change-region"),
        "the dialog stays while it saves"
    );
    gate.open();

    h.settle_with(|h| !h.has("dlg-change-region"));
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "eu-west-1");
    assert_eq!(
        h.text_of(&format!("cell-region-{id}")).as_deref(),
        Some("EU-WEST-1")
    );
}

#[test]
fn a_failed_change_shows_its_error_and_the_saved_state() {
    let (mut h, id) = with_session();
    menu(&mut h, id, "Change named profile…");
    h.tap("sel-change-profile-value-trigger");
    h.type_text("bad[name");
    h.frames(10);
    h.tap(NEW_PROFILE_OPTION);
    h.tap("btn-change-profile-save");
    h.settle(|ui| ui.error().is_some());

    assert!(h.has("toast-error"));
    assert!(
        h.tree_contains("must not contain [, ], or line breaks"),
        "the toast shows the core error"
    );
    assert!(
        h.has("dlg-change-profile"),
        "a name error keeps the dialog open"
    );
    assert!(
        !h.is_disabled("btn-change-profile-save"),
        "save works again"
    );
    assert_eq!(
        h.text_of(&format!("cell-profile-{id}")).as_deref(),
        Some("default")
    );
}

#[test]
fn a_change_of_a_deleted_session_shows_the_error_and_closes() {
    let (mut h, id) = with_session();
    menu(&mut h, id, "Change region…");
    choose_region(&mut h, "eu-west-1");
    let leapp = h.leapp.clone();
    h.block_on(leapp.delete_session(id)).unwrap();

    h.tap("btn-change-region-save");
    h.settle(|ui| ui.error().is_some());

    assert!(h.has("toast-error"));
    assert!(!h.has("dlg-change-region"));
    assert!(
        !h.has(&format!("row-{id}")),
        "the UI reads the workspace again"
    );
}
