mod common;

use common::AppHarness;
use leapp_core::regions::REGIONS;

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
    let rows = h.keys_with_prefix("row-");
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
    h.tap("fld-profile");
    h.type_text("[bad]");
    h.tap("btn-create-submit");

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
fn a_chosen_region_is_used() {
    let mut h = AppHarness::new();
    h.tap("btn-create");
    h.tap("sel-region-trigger");
    h.frames(30);
    // The list scrolls; the first options are the ones in view.
    let region = REGIONS[1];
    assert_ne!(region, "us-east-1");
    h.tap("option1");
    fill(&mut h, "dev", "AKID-dev", "SECRET-dev");
    h.tap("btn-create-submit");
    h.settle(|ui| ui.workspace().sessions.len() == 1);

    assert_eq!(h.leapp.workspace().sessions[0].region, region);
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
