mod common;

use common::{AppHarness, iam_user};
use leapp_app::Msg;
use leapp_core::{Error, SessionStatus};

#[test]
fn empty_workspace_shows_the_empty_state() {
    let mut h = AppHarness::new();
    h.frames(2);

    assert!(h.has("empty-state"));
    assert_eq!(h.text_of("empty-state").as_deref(), Some("No sessions yet"));
    assert!(h.row_keys().is_empty());
    assert!(h.has("sidebar"));
    assert_eq!(
        h.text_of("nav-all-sessions").as_deref(),
        Some("All Sessions")
    );
    assert!(h.has("btn-create"));
}

#[test]
fn existing_sessions_show_as_rows() {
    let (mut h, id) =
        AppHarness::with_setup(|leapp| leapp.create_iam_user(iam_user("dev", "ci")).unwrap());
    h.frames(2);

    assert!(!h.has("empty-state"));
    assert!(h.has("table"));
    assert_eq!(h.row_keys(), vec![format!("row-{id}")]);
    assert_eq!(
        h.text_of(&format!("cell-name-{id}")).as_deref(),
        Some("dev")
    );
    assert_eq!(
        h.text_of(&format!("cell-provider-{id}")).as_deref(),
        Some("IAM User")
    );
    assert_eq!(
        h.text_of(&format!("cell-profile-{id}")).as_deref(),
        Some("ci")
    );
    assert_eq!(
        h.text_of(&format!("cell-region-{id}")).as_deref(),
        Some("EU-WEST-1")
    );
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("inactive")
    );
}

#[test]
fn active_sessions_are_listed_first() {
    let (mut h, (a, b)) = AppHarness::with_setup(|leapp| {
        let a = leapp.create_iam_user(iam_user("a-first", "p-a")).unwrap();
        let b = leapp.create_iam_user(iam_user("b-second", "p-b")).unwrap();
        (a, b)
    });
    h.frames(2);
    let (row_a, row_b) = (format!("row-{a}"), format!("row-{b}"));
    assert!(h.rect(&row_a)[1] < h.rect(&row_b)[1], "sorted by name");

    let leapp = h.leapp.clone();
    h.block_on(leapp.start(b)).unwrap();
    h.bridge.send(Msg::Refreshed);
    h.settle(|ui| ui.workspace().session(b).unwrap().status == SessionStatus::Active);
    h.frames(60);

    assert!(h.rect(&row_b)[1] < h.rect(&row_a)[1], "active first");
    assert_eq!(h.text_of(&format!("status-{b}")).as_deref(), Some("active"));
    assert_eq!(
        h.text_of(&format!("status-{a}")).as_deref(),
        Some("inactive")
    );
}

#[test]
fn a_failed_operation_shows_its_error_until_closed() {
    let mut h = AppHarness::new();
    assert!(!h.has("toast-error"));

    h.rt.state
        .run(async { Err(Error::Aws("The security token is not valid".into())) });
    h.settle(|ui| ui.error().is_some());

    assert_eq!(
        h.text_of("toast-error").as_deref(),
        Some("The security token is not valid")
    );

    h.tap("toast-error-close");
    assert!(!h.has("toast-error"));
}

#[test]
fn the_one_session_scene_shows_one_inactive_row() {
    let mut h = AppHarness::new();
    let (_, scene) = leapp_app::spec()
        .scenes
        .iter()
        .find(|(name, _)| *name == "one-session")
        .copied()
        .expect("scene one-session");

    scene(&mut h.rt.state);
    h.rt.update(|_, _| ());
    h.frames(2);

    let rows = h.row_keys();
    assert_eq!(rows.len(), 1);
    let id = rows[0].strip_prefix("row-").unwrap();
    assert_eq!(
        h.text_of(&format!("status-{id}")).as_deref(),
        Some("inactive")
    );
    assert_eq!(
        h.text_of(&format!("cell-provider-{id}")).as_deref(),
        Some("IAM User")
    );
}

#[test]
fn the_narrowest_window_keeps_the_table_headers_apart() {
    let (mut h, id) = AppHarness::with_setup(|leapp| {
        leapp
            .create_iam_user(common::iam_user("A session with a long name", "default"))
            .unwrap()
    });
    let (min_w, min_h) = leapp_app::spec()
        .min_size
        .expect("the window has a minimum size");
    h.rt.set_viewport((min_w, min_h), 1.0);
    h.frames(2);

    let session = h.rect("hdr-session");
    let role = h.rect("hdr-role");
    assert!(
        session[0] + session[2] <= role[0],
        "Session {session:?} runs into Role {role:?}"
    );
    // 160 is `SESSION_MIN_WIDTH` in `views/main_window.rs`.
    assert!(
        session[2] >= 160.0,
        "Session column too narrow: {session:?}"
    );
    let status = h.rect(&format!("status-{id}"));
    assert!(
        status[0] + status[2] <= min_w,
        "the status dot {status:?} is outside the {min_w} px window"
    );
}

#[test]
fn the_empty_state_has_an_add_session_button() {
    let mut h = AppHarness::new();
    assert_eq!(
        h.text_of("btn-empty-add-session").as_deref(),
        Some("Add session")
    );
    assert!(!h.has("dlg-iam-user"));

    h.tap("btn-empty-add-session");

    assert!(h.has("dlg-iam-user"), "the create form is open");
    assert!(h.rt.state.create_form().is_open());
}

#[test]
fn a_starting_session_stays_pending_through_other_updates() {
    let (mut h, id) =
        AppHarness::with_setup(|leapp| leapp.create_iam_user(iam_user("dev", "ci")).unwrap());
    h.fake
        .delay_next("GetSessionToken", std::time::Duration::from_millis(300));
    let status = format!("status-{id}");
    let (x, y) = h.rt.find(&format!("row-{id}")).unwrap();
    // No task runs until the refresh below: core has not marked the start when the UI reads
    // the workspace again.
    let workers = h.hold_workers();
    for _ in 0..2 {
        h.rt.pointer_down(x - 100.0, y);
        h.rt.pointer_up(x - 100.0, y);
        h.frames(1);
    }
    assert_eq!(h.text_of(&status).as_deref(), Some("pending"));
    // Another update reads the workspace at once, before core marks the start.
    h.bridge.send(Msg::Refreshed);
    h.frames(1);

    assert_eq!(h.text_of(&status).as_deref(), Some("pending"));
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive,
        "core has not marked the start yet"
    );
    workers.open();
    h.settle(|ui| ui.workspace().session(id).unwrap().status == SessionStatus::Active);
    assert_eq!(h.text_of(&status).as_deref(), Some("active"));
}
