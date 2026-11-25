//! The type chooser of "Add session" and the IAM Role Chained create form.

mod common;

use common::{AppHarness, iam_user};
use leapp_core::{Leapp, NewIamRoleChained, SessionId, SessionKind, SessionStatus};
use rune::Key;

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

/// Opens the row menu of `id` and chooses Edit session.
fn open_edit(h: &mut AppHarness, id: SessionId) {
    h.tap(&format!("row-menu-{id}"));
    h.tap_menu_item(id, "Edit session");
    assert!(h.has("dlg-iam-role-chained"), "the chained form opens");
}

/// Focuses `field` and deletes its text.
fn clear(h: &mut AppHarness, field: &str) {
    h.tap(field);
    h.press(Key::End);
    for _ in 0..80 {
        h.press(Key::Backspace);
    }
}

fn role_arn(h: &AppHarness, id: SessionId) -> String {
    match &h.leapp.workspace().session(id).unwrap().kind {
        SessionKind::IamRoleChained { role_arn, .. } => role_arn.clone(),
        SessionKind::IamUser { .. } => panic!("not a chained session"),
    }
}

/// A harness with one IAM User `P` on the `default` profile: the parent.
fn with_parent() -> (AppHarness, SessionId) {
    AppHarness::with_setup(|l| l.create_iam_user(iam_user("P", "default")).unwrap())
}

fn credentials(h: &AppHarness) -> String {
    std::fs::read_to_string(h.home.path().join(".aws/credentials")).unwrap_or_default()
}

fn type_into(h: &mut AppHarness, key: &str, text: &str) {
    h.tap(key);
    h.type_text(text);
}

fn choose_parent(h: &mut AppHarness, parent: SessionId) {
    h.tap("sel-parent-value-trigger");
    h.frames(10);
    h.tap(&format!("sel-parent-option-{parent}"));
}

/// Opens the chained form and fills its required fields.
fn fill_required(h: &mut AppHarness, parent: SessionId, role_arn: &str) {
    h.open_chained_form();
    type_into(h, "fld-chained-name", "deploy");
    choose_parent(h, parent);
    type_into(h, "fld-role-arn", role_arn);
}

fn submit_until(h: &mut AppHarness, key: &str) {
    h.tap("btn-chained-submit");
    h.settle_with(|h| h.has(key));
}

#[test]
fn add_session_asks_for_the_type() {
    let mut h = AppHarness::new();
    h.tap("btn-create");

    assert!(h.has("dlg-choose-type"));
    assert!(!h.has("dlg-iam-user"), "no form before the type is chosen");
    assert_eq!(
        h.text_of("btn-type-iam-user").as_deref(),
        Some("AWS IAM User")
    );
    assert_eq!(
        h.text_of("btn-type-iam-role-chained").as_deref(),
        Some("AWS IAM Role Chained")
    );
    // F23-R12: Single Sign-On comes with F4.
    assert!(!h.tree_contains("Single Sign-On"));

    h.tap("btn-type-iam-user");
    assert!(h.has("dlg-iam-user"));
    assert!(!h.has("dlg-choose-type"), "the chooser closes");

    h.tap("btn-create-cancel");
    h.tap("btn-create");
    h.tap("btn-type-iam-role-chained");
    assert!(h.has("dlg-iam-role-chained"));
    assert!(!h.has("dlg-choose-type"));
    assert!(!h.has("dlg-iam-user"));
}

#[test]
fn a_chained_session_is_created_and_started() {
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    h.tap("sel-profile-value-trigger");
    h.type_text("ci");
    h.frames(10);
    h.tap("sel-profile-option-__new__");
    h.tap("btn-chained-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 2);

    assert!(!h.has("dlg-iam-role-chained"), "the form closes");
    let ws = h.leapp.workspace();
    let chained = ws.sessions.iter().find(|s| s.name == "deploy").unwrap();
    let id = chained.id;
    let SessionKind::IamRoleChained {
        parent_session_id,
        role_arn,
        role_session_name,
        external_id,
    } = &chained.kind
    else {
        panic!("not a chained session");
    };
    assert_eq!(*parent_session_id, parent);
    assert_eq!(role_arn, ROLE_ARN);
    assert_eq!(*role_session_name, None, "an empty field is not set");
    assert_eq!(*external_id, None);
    assert_eq!(chained.region, "us-east-1");

    assert_eq!(
        h.text_of(&format!("cell-provider-{id}")).as_deref(),
        Some("IAM Role Chained")
    );
    assert_eq!(
        h.text_of(&format!("cell-profile-{id}")).as_deref(),
        Some("ci")
    );

    h.tap(&format!("status-{id}"));
    h.settle(|ui| ui.workspace().session(id).unwrap().status == SessionStatus::Active);
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("active")
    );
    let file = credentials(&h);
    assert!(file.contains("[ci]"), "{file}");
    assert!(file.contains("fake-chained-token-1"), "{file}");
}

#[test]
fn the_optional_fields_are_saved() {
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    assert_eq!(
        h.placeholder_of("fld-role-session-name").as_deref(),
        Some("assumed-from-leapp")
    );
    type_into(&mut h, "fld-role-session-name", "ci-run");
    type_into(&mut h, "fld-external-id", "ext-1");
    h.tap("btn-chained-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 2);

    let ws = h.leapp.workspace();
    let chained = ws.sessions.iter().find(|s| s.name == "deploy").unwrap();
    let SessionKind::IamRoleChained {
        role_session_name,
        external_id,
        ..
    } = &chained.kind
    else {
        panic!("not a chained session");
    };
    assert_eq!(role_session_name.as_deref(), Some("ci-run"));
    assert_eq!(external_id.as_deref(), Some("ext-1"));
}

#[test]
fn chained_errors_show_under_the_field() {
    // A bad role ARN.
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, "deploy");
    submit_until(&mut h, "err-role-arn");
    assert!(
        h.text_of("err-role-arn")
            .unwrap()
            .contains("must look like arn:aws:iam::123456789012:role/name")
    );
    assert!(h.has("dlg-iam-role-chained"), "the form stays open");
    assert_eq!(h.leapp.workspace().sessions.len(), 1, "nothing is added");
    assert!(!h.has("err-chained-name") && !h.has("err-parent") && !h.has("err-chained-form"));
    // Typing in the field clears its error.
    type_into(&mut h, "fld-role-arn", "x");
    assert!(!h.has("err-role-arn"));

    // A bad role session name.
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    type_into(&mut h, "fld-role-session-name", "x");
    submit_until(&mut h, "err-role-session-name");
    assert!(!h.has("err-role-arn") && !h.has("err-chained-form"));
    assert_eq!(h.leapp.workspace().sessions.len(), 1);

    // A bad external ID.
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    type_into(&mut h, "fld-external-id", "x");
    submit_until(&mut h, "err-external-id");
    assert!(!h.has("err-role-session-name") && !h.has("err-chained-form"));

    // A bad profile name.
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    h.tap("sel-profile-value-trigger");
    h.type_text("[bad]");
    h.frames(10);
    h.tap("sel-profile-option-__new__");
    submit_until(&mut h, "err-profile");
    assert!(!h.has("err-chained-form"));
}

#[test]
fn the_submit_waits_for_the_required_fields() {
    // Each check leaves out one required field: name, then parent, then role ARN.
    let (mut h, parent) = with_parent();
    h.open_chained_form();
    assert!(h.is_disabled("btn-chained-submit"), "an empty form");
    choose_parent(&mut h, parent);
    type_into(&mut h, "fld-role-arn", ROLE_ARN);
    type_into(&mut h, "fld-chained-name", "   ");
    assert!(h.is_disabled("btn-chained-submit"), "no name");
    type_into(&mut h, "fld-chained-name", "deploy");
    assert!(!h.is_disabled("btn-chained-submit"), "all three are set");

    let (mut h, _) = with_parent();
    h.open_chained_form();
    type_into(&mut h, "fld-chained-name", "deploy");
    type_into(&mut h, "fld-role-arn", ROLE_ARN);
    assert!(h.is_disabled("btn-chained-submit"), "no parent");
    // A tap on the disabled button adds nothing.
    h.tap("btn-chained-submit");
    h.frames(10);
    assert_eq!(h.leapp.workspace().sessions.len(), 1);

    let (mut h, parent) = with_parent();
    h.open_chained_form();
    type_into(&mut h, "fld-chained-name", "deploy");
    choose_parent(&mut h, parent);
    assert!(h.is_disabled("btn-chained-submit"), "no role ARN");
    type_into(&mut h, "fld-role-arn", "   ");
    assert!(
        h.is_disabled("btn-chained-submit"),
        "spaces are not a role ARN"
    );
    type_into(&mut h, "fld-role-arn", "arn");
    assert!(!h.is_disabled("btn-chained-submit"));
}

#[test]
fn the_parent_list_has_every_session() {
    let (mut h, (p, q)) = AppHarness::with_setup(|l| {
        let p = l.create_iam_user(iam_user("P", "default")).unwrap();
        let q = l.create_iam_user(iam_user("Q", "dev")).unwrap();
        (p, q)
    });
    h.open_chained_form();
    h.tap("sel-parent-value-trigger");
    h.frames(10);

    let mut shown = h.keys_with_prefix("sel-parent-option-");
    shown.sort();
    let mut want = vec![
        format!("sel-parent-option-{p}"),
        format!("sel-parent-option-{q}"),
    ];
    want.sort();
    assert_eq!(shown, want);
    assert_eq!(
        h.text_of(&format!("sel-parent-option-{q}")).as_deref(),
        Some("Q")
    );
}

#[test]
fn cancel_closes_the_form_and_wipes_it() {
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    h.tap("btn-chained-cancel");
    assert!(!h.has("dlg-iam-role-chained"));
    assert_eq!(h.leapp.workspace().sessions.len(), 1);

    h.open_chained_form();
    assert!(
        h.is_disabled("btn-chained-submit"),
        "the fields are empty again"
    );
}

#[test]
fn the_form_fits_the_smallest_window() {
    let (mut h, _) = with_parent();
    let (min_w, min_h) = leapp_app::spec().min_size.unwrap();
    h.rt.set_viewport((min_w, min_h), 1.0);
    h.frames(2);
    h.open_chained_form();
    h.frames(30);
    for key in ["btn-chained-submit", "btn-chained-cancel", "sel-region"] {
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
    let (mut h, parent) = with_parent();
    fill_required(&mut h, parent, ROLE_ARN);
    // The result of the create waits for a worker: the form stays in its saving state.
    let workers = h.hold_workers();
    h.tap("btn-chained-submit");
    h.frames(5);
    assert!(h.has("dlg-iam-role-chained"));
    assert!(h.is_disabled("btn-chained-submit"));
    assert!(h.is_disabled("btn-chained-cancel"));
    h.tap("btn-chained-submit");
    workers.open();
    h.settle_with(|h| !h.has("dlg-iam-role-chained"));
    h.frames(5);
    assert_eq!(
        h.leapp.workspace().sessions.len(),
        2,
        "one tap, one session"
    );
}

#[test]
fn a_chained_session_is_edited() {
    const NEW_ARN: &str = "arn:aws:iam::210987654321:role/release";
    let (mut h, (parent, id)) = AppHarness::with_setup(|l| {
        let p = l.create_iam_user(iam_user("P", "default")).unwrap();
        let c = chained(l, "deploy", p, "ci");
        (p, c)
    });
    h.tap(&format!("status-{id}"));
    h.settle(|ui| ui.workspace().session(id).unwrap().status == SessionStatus::Active);

    open_edit(&mut h, id);
    assert!(h.has_text("Edit IAM Role Chained"), "the title");
    assert_eq!(h.text_of("btn-chained-submit").as_deref(), Some("Save"));
    assert_eq!(h.text_of("fld-chained-name").as_deref(), Some("deploy"));
    assert_eq!(h.text_of("fld-role-arn").as_deref(), Some(ROLE_ARN));
    assert_eq!(h.text_of("sel-parent").as_deref(), Some("P"), "the parent");

    clear(&mut h, "fld-role-arn");
    h.type_text(NEW_ARN);
    h.tap("btn-chained-submit");
    h.settle_with(|h| !h.has("dlg-iam-role-chained"));
    h.settle(|ui| ui.workspace().session(id).unwrap().status == SessionStatus::Active);

    assert_eq!(role_arn(&h, id), NEW_ARN);
    let ws = h.leapp.workspace();
    let session = ws.session(id).unwrap();
    assert_eq!(
        session.status,
        SessionStatus::Active,
        "active after the restart"
    );
    assert_eq!(session.name, "deploy", "the other fields stay");
    assert!(matches!(
        &session.kind,
        SessionKind::IamRoleChained { parent_session_id, .. } if *parent_session_id == parent
    ));
    assert_eq!(ws.sessions.len(), 2, "no new session");
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("active")
    );
    // The restart assumed the new role.
    let last = h
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.action == "AssumeRole")
        .unwrap();
    assert_eq!(
        last.params.get("RoleArn").map(String::as_str),
        Some(NEW_ARN)
    );
    assert_eq!(h.fake.count("AssumeRole"), 2, "one start, one restart");
}

#[test]
fn the_parent_select_leaves_out_the_session_and_its_chain() {
    let (mut h, (p, c1, c2)) = AppHarness::with_setup(|l| {
        let p = l.create_iam_user(iam_user("P", "default")).unwrap();
        let c1 = chained(l, "C1", p, "c1");
        let c2 = chained(l, "C2", c1, "c2");
        (p, c1, c2)
    });
    open_edit(&mut h, c1);
    h.tap("sel-parent-value-trigger");
    h.frames(10);

    assert_eq!(
        h.keys_with_prefix("sel-parent-option-"),
        [format!("sel-parent-option-{p}")]
    );
    assert!(!h.has(&format!("sel-parent-option-{c1}")));
    assert!(!h.has(&format!("sel-parent-option-{c2}")));

    // The create form still lists every session.
    h.press(Key::Escape);
    h.tap("btn-chained-cancel");
    h.open_chained_form();
    h.tap("sel-parent-value-trigger");
    h.frames(10);
    assert_eq!(h.keys_with_prefix("sel-parent-option-").len(), 3);
}
