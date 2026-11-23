mod common;

use common::Harness;
use leapp_core::{Config, Error, Leapp, SessionKind, SessionStatus};

#[tokio::test]
async fn created_session_is_saved_and_survives_reopen() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    let ws = h.leapp.workspace();
    assert_eq!(ws.sessions.len(), 1);
    let s = ws.session(id).unwrap();
    assert_eq!(s.name, "dev");
    assert_eq!(s.region, "eu-west-1");
    assert_eq!(s.status, SessionStatus::Inactive);
    assert!(matches!(
        s.kind,
        SessionKind::IamUser {
            mfa_device: None,
            ..
        }
    ));
    assert_eq!(ws.profile_name(s.profile_id), Some("default"));

    let h = h.reopen();
    let ws = h.leapp.workspace();
    assert_eq!(ws.sessions.len(), 1);
    assert_eq!(ws.session(id).unwrap().name, "dev");
}

#[tokio::test]
async fn keys_go_to_the_secret_store_not_the_file() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    let snap = h.secrets.snapshot();
    assert_eq!(snap[&format!("iam-user-{id}-access-key-id")], "AKID-dev");
    assert_eq!(
        snap[&format!("iam-user-{id}-secret-access-key")],
        "SECRET-dev"
    );
    let text = h.workspace_text();
    assert!(!text.contains("AKID-dev"));
    assert!(!text.contains("SECRET-dev"));
}

#[tokio::test]
async fn new_profile_name_creates_a_profile_once() {
    let h = Harness::new().await;
    h.leapp.create_iam_user(h.iam_user("a", "ci")).unwrap();
    h.leapp.create_iam_user(h.iam_user("b", "ci")).unwrap();

    let names: Vec<_> = h
        .leapp
        .workspace()
        .profiles
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, ["default", "ci"]);
}

#[tokio::test]
async fn default_profile_exists_in_a_new_workspace() {
    let h = Harness::new().await;
    let names: Vec<_> = h
        .leapp
        .workspace()
        .profiles
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, ["default"]);
}

#[tokio::test]
async fn rejects_bad_profile_names() {
    let h = Harness::new().await;
    for p in ["", "   ", "a[b", "a]b", "a\nb", "a\rb"] {
        let err = h.leapp.create_iam_user(h.iam_user("dev", p)).unwrap_err();
        assert!(
            matches!(
                err,
                Error::InvalidInput {
                    field: "profile",
                    ..
                }
            ),
            "profile {p:?} gave {err:?}"
        );
    }
    assert!(h.leapp.workspace().sessions.is_empty());
}

#[tokio::test]
async fn rejects_missing_fields_and_unknown_region() {
    let h = Harness::new().await;
    let field = |req| match h.leapp.create_iam_user(req).unwrap_err() {
        Error::InvalidInput { field, .. } => field,
        other => panic!("unexpected error {other:?}"),
    };

    let mut r = h.iam_user("dev", "default");
    r.name = "  ".into();
    assert_eq!(field(r), "name");
    let mut r = h.iam_user("dev", "default");
    r.access_key_id = String::new();
    assert_eq!(field(r), "access_key_id");
    let mut r = h.iam_user("dev", "default");
    r.secret_access_key = String::new();
    assert_eq!(field(r), "secret_access_key");
    let mut r = h.iam_user("dev", "default");
    r.region = "mars-1".into();
    assert_eq!(field(r), "region");
    assert!(h.secrets.snapshot().is_empty());
}

#[tokio::test]
async fn newer_version_file_is_refused_and_not_overwritten() {
    let h = Harness::new().await;
    let path = h.workspace_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = r#"{"version": 99, "sessions": []}"#;
    std::fs::write(&path, bytes).unwrap();

    let config = Config::new(
        h.home.path().to_path_buf(),
        h.secrets.clone(),
        h.prompter.clone(),
    );
    let err = Leapp::open(config).err().unwrap();
    assert!(matches!(err, Error::UnsupportedVersion(99)), "{err:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
}

#[tokio::test]
async fn broken_json_file_is_moved_aside() {
    let h = Harness::new().await;
    let path = h.workspace_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();

    let config = Config::new(
        h.home.path().to_path_buf(),
        h.secrets.clone(),
        h.prompter.clone(),
    );
    let leapp = Leapp::open(config).unwrap();
    assert!(leapp.workspace().sessions.is_empty());

    let moved: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("workspace.json.broken-")
        })
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(std::fs::read_to_string(&moved[0]).unwrap(), "{not json");
}

#[tokio::test]
async fn debug_output_of_new_iam_user_hides_the_keys() {
    let h = Harness::new().await;
    let text = format!("{:?}", h.iam_user("dev", "default"));
    assert!(!text.contains("SECRET-dev"), "{text}");
    assert!(!text.contains("AKID-dev"), "{text}");
    assert!(text.contains("dev"), "{text}");
}

#[tokio::test]
async fn failed_save_leaves_no_secrets_and_no_session() {
    let h = Harness::new().await;
    // A folder at the workspace path makes the final rename fail on macOS and Linux.
    std::fs::create_dir_all(h.workspace_path()).unwrap();

    let before = h.leapp.workspace();
    let result = h.leapp.create_iam_user(h.iam_user("dev", "newprofile"));

    assert!(result.is_err());
    assert!(h.secrets.snapshot().is_empty());
    let ws = h.leapp.workspace();
    assert!(ws.sessions.is_empty());
    assert_eq!(ws.profiles, before.profiles);
}
