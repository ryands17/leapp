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
    assert!(h.keys_with_prefix("row-").is_empty());
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
    assert_eq!(h.keys_with_prefix("row-"), vec![format!("row-{id}")]);
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

    let rows = h.keys_with_prefix("row-");
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
    assert!(
        session[2] >= 120.0,
        "Session column too narrow: {session:?}"
    );
    let status = h.rect(&format!("status-{id}"));
    assert!(
        status[0] + status[2] <= min_w,
        "the status dot {status:?} is outside the {min_w} px window"
    );
}
