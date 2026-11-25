//! The row menu: start and stop, change region and profile, copy account number, edit, delete.

mod common;

use common::{AppHarness, iam_user};
use leapp_core::{Leapp, NewIamRoleChained, SessionId, SessionStatus};

const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/deploy";

/// A chained session `name` below `parent`, on profile `profile`, that assumes [`ROLE_ARN`].
fn chained(l: &Leapp, name: &str, parent: SessionId, profile: &str) -> SessionId {
    l.create_iam_role_chained(NewIamRoleChained {
        name: name.to_string(),
        parent_session_id: parent,
        role_arn: ROLE_ARN.to_string(),
        role_session_name: None,
        external_id: None,
        profile_name: profile.to_string(),
        region: "eu-west-1".to_string(),
    })
    .unwrap()
}

/// The texts of the items of the open menu of `id`, top to bottom.
fn menu_items(h: &AppHarness, id: SessionId) -> Vec<String> {
    h.keys_with_prefix(&format!("menu-{id}-item"))
        .iter()
        .map(|k| h.text_of(k).unwrap_or_default())
        .collect()
}

fn status(ui: &leapp_app::Ui, id: SessionId) -> SessionStatus {
    ui.workspace().session(id).unwrap().status
}

/// A harness with one IAM User `dev` on profile `default`.
fn with_session() -> (AppHarness, SessionId) {
    AppHarness::with_setup(|l| l.create_iam_user(iam_user("dev", "default")).unwrap())
}

/// Opens the row menu of `id` with its `⋯` button.
fn open_menu(h: &mut AppHarness, id: SessionId) {
    h.tap(&format!("row-menu-{id}"));
    assert!(h.has(&format!("menu-{id}-menu")), "the menu is open");
}

fn start(h: &mut AppHarness, id: SessionId) {
    open_menu(h, id);
    h.tap_menu_item(id, "Start session");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

fn stop(h: &mut AppHarness, id: SessionId) {
    open_menu(h, id);
    h.tap_menu_item(id, "Stop session");
    h.settle(|ui| status(ui, id) == SessionStatus::Inactive);
}

#[test]
fn the_menu_starts_and_stops() {
    let (mut h, id) = with_session();
    let status_key = format!("status-{id}");

    open_menu(&mut h, id);
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
    h.tap_menu_item(id, "Start session");
    assert!(
        !h.has(&format!("menu-{id}-menu")),
        "a choice closes the menu"
    );
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
    assert_eq!(h.text_of(&status_key).as_deref(), Some("active"));
    assert!(h.leapp.workspace().session(id).unwrap().status == SessionStatus::Active);

    open_menu(&mut h, id);
    assert!(
        h.keys_with_prefix(&format!("menu-{id}-item"))
            .iter()
            .any(|k| h.text_of(k).as_deref() == Some("Stop session")),
        "an active session's menu offers Stop session"
    );
    h.tap_menu_item(id, "Stop session");
    h.settle(|ui| status(ui, id) == SessionStatus::Inactive);
    assert_eq!(h.text_of(&status_key).as_deref(), Some("inactive"));
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
}

#[test]
fn right_click_opens_the_same_menu() {
    let (mut h, id) = with_session();
    assert!(!h.has(&format!("menu-{id}-menu")));

    h.secondary_tap(&format!("row-{id}"));

    assert!(h.has(&format!("menu-{id}-menu")));
    h.tap_menu_item(id, "Start session");
    h.settle(|ui| status(ui, id) == SessionStatus::Active);
}

#[test]
fn copy_account_number_copies_it() {
    let (mut h, id) = with_session();
    start(&mut h, id);
    stop(&mut h, id);

    open_menu(&mut h, id);
    h.tap_menu_item(id, "Copy account number");
    h.frames(2);

    assert_eq!(h.clipboard_text().as_deref(), Some("123456789012"));
}

#[test]
fn copy_account_number_is_disabled_before_the_first_start() {
    let (mut h, id) = with_session();
    open_menu(&mut h, id);
    let item = h
        .keys_with_prefix(&format!("menu-{id}-item"))
        .into_iter()
        .find(|k| h.text_of(k).as_deref() == Some("Copy account number"))
        .unwrap();
    assert!(h.is_disabled(&item));

    h.tap(&item);
    h.frames(2);

    assert_eq!(h.clipboard_text(), None);
}

#[test]
fn delete_asks_then_deletes() {
    let (mut h, id) = with_session();
    let row = format!("row-{id}");

    open_menu(&mut h, id);
    h.tap_menu_item(id, "Delete session");
    assert_eq!(
        h.text_of("dlg-delete").as_deref(),
        Some("Delete session dev?")
    );
    assert_eq!(h.text_of("btn-delete-yes").as_deref(), Some("Delete"));
    assert_eq!(h.text_of("btn-delete-no").as_deref(), Some("Cancel"));

    h.wait_for_arming();
    h.tap("btn-delete-no");
    h.frames(30);
    assert!(!h.has("dlg-delete"));
    assert!(h.has(&row), "Cancel keeps the session");
    assert!(h.leapp.workspace().session(id).is_some());

    open_menu(&mut h, id);
    h.tap_menu_item(id, "Delete session");
    h.wait_for_arming();
    h.tap("btn-delete-yes");
    h.settle(|ui| ui.workspace().session(id).is_none());

    assert!(!h.has("dlg-delete"));
    assert!(!h.has(&row));
    assert!(h.has("empty-state"));
    assert!(h.leapp.workspace().session(id).is_none());
    let prefix = format!("iam-user-{id}-");
    assert!(
        !h.secrets.snapshot().keys().any(|k| k.starts_with(&prefix)),
        "the keychain has no key of the session"
    );
}

#[test]
fn delete_ignores_answers_until_armed() {
    let (mut h, id) = with_session();
    open_menu(&mut h, id);
    h.tap_menu_item(id, "Delete session");

    h.tap("btn-delete-yes");
    h.frames(3);
    assert!(h.has("dlg-delete"), "an early answer is ignored");
    assert!(h.leapp.workspace().session(id).is_some());
    assert!(h.is_disabled("btn-delete-yes"));
    assert!(h.is_disabled("btn-delete-no"));

    h.wait_for_arming();
    assert!(!h.is_disabled("btn-delete-yes"));
    h.tap("btn-delete-yes");
    h.settle(|ui| ui.workspace().session(id).is_none());
}

#[test]
fn a_failed_delete_shows_its_error() {
    let (mut h, id) = with_session();
    open_menu(&mut h, id);
    h.tap_menu_item(id, "Delete session");
    // Core deletes the session first, so the UI's delete finds it gone.
    let leapp = h.leapp.clone();
    h.block_on(leapp.delete_session(id)).unwrap();

    h.wait_for_arming();
    h.tap("btn-delete-yes");
    h.settle(|ui| ui.error().is_some());

    assert!(h.has("toast-error"));
    assert!(!h.has(&format!("row-{id}")));
}

#[test]
fn edit_opens_the_form_for_that_session() {
    let (mut h, (_, b)) = AppHarness::with_setup(|l| {
        let a = l.create_iam_user(iam_user("alpha", "pa")).unwrap();
        let b = l.create_iam_user(iam_user("beta", "pb")).unwrap();
        (a, b)
    });

    open_menu(&mut h, b);
    h.tap_menu_item(b, "Edit session");

    assert!(h.has("dlg-iam-user"));
    assert!(h.has_text("Edit IAM User"), "the title");
    assert_eq!(h.rt.state.create_form().name(), "beta");
    assert_eq!(h.text_of("fld-name").as_deref(), Some("beta"));
}

#[test]
fn copy_role_arn_and_account_of_a_chained_session() {
    let (mut h, (p, c)) = AppHarness::with_setup(|l| {
        let p = l.create_iam_user(iam_user("P", "default")).unwrap();
        let c = chained(l, "C", p, "ci");
        (p, c)
    });

    open_menu(&mut h, c);
    assert_eq!(
        menu_items(&h, c),
        [
            "Start session",
            "Change region…",
            "Change named profile…",
            "Copy account number",
            "Copy role ARN",
            "Edit session",
            "Delete session"
        ]
    );
    h.tap_menu_item(c, "Copy role ARN");
    h.frames(2);
    assert_eq!(h.clipboard_text().as_deref(), Some(ROLE_ARN));

    // The account comes from the role ARN: no start is needed.
    open_menu(&mut h, c);
    h.tap_menu_item(c, "Copy account number");
    h.frames(2);
    assert_eq!(h.clipboard_text().as_deref(), Some("123456789012"));

    open_menu(&mut h, p);
    assert!(
        !menu_items(&h, p).iter().any(|i| i == "Copy role ARN"),
        "an IAM User has no role ARN"
    );
}

#[test]
fn delete_of_a_parent_names_its_chain() {
    let (mut h, (p, c1, c2)) = AppHarness::with_setup(|l| {
        let p = l.create_iam_user(iam_user("P", "default")).unwrap();
        let c1 = chained(l, "C1", p, "c1");
        let c2 = chained(l, "C2", c1, "c2");
        (p, c1, c2)
    });

    // One session below: the singular form.
    open_menu(&mut h, c1);
    h.tap_menu_item(c1, "Delete session");
    assert_eq!(
        h.text_of("dlg-delete").as_deref(),
        Some("Delete session C1? 1 chained session that uses it will also be deleted.")
    );
    h.wait_for_arming();
    h.tap("btn-delete-no");
    h.frames(30);

    open_menu(&mut h, p);
    h.tap_menu_item(p, "Delete session");
    assert_eq!(
        h.text_of("dlg-delete").as_deref(),
        Some("Delete session P? 2 chained sessions that use it will also be deleted.")
    );
    h.wait_for_arming();
    h.tap("btn-delete-yes");
    h.settle(|ui| ui.workspace().sessions.is_empty());

    for id in [p, c1, c2] {
        assert!(!h.has(&format!("row-{id}")), "the row of {id} is gone");
    }
    assert!(h.leapp.workspace().sessions.is_empty());
}
