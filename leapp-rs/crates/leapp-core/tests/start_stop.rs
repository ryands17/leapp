mod common;

use std::sync::Arc;

use common::{FlakyStore, Harness, block_on};
use leapp_core::{Error, SessionStatus};

const DEV_ON_DEFAULT: &str = "[default]\naws_access_key_id = ASIAFAKE0001\naws_secret_access_key = fake-secret-1\naws_session_token = fake-token-1\nregion = eu-west-1\n";

#[tokio::test]
async fn start_writes_the_profile_section_with_temporary_credentials() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    assert_eq!(s.status, SessionStatus::Active);
    assert!(s.start_time.is_some());
    assert_eq!(h.credentials_text().as_deref(), Some(DEV_ON_DEFAULT));
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    let call = &h.fake.requests()[0];
    assert_eq!(
        call.params.get("DurationSeconds").map(String::as_str),
        Some("36000")
    );
    assert_eq!(call.access_key_id.as_deref(), Some("AKID-dev"));
}

#[tokio::test]
async fn stop_removes_the_section_and_sets_inactive() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    h.leapp.stop(id).await.unwrap();

    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    assert_eq!(s.status, SessionStatus::Inactive);
    assert!(s.start_time.is_none());
    assert_eq!(h.credentials_text().as_deref(), Some(""));
    assert!(ws.managed_profiles.is_empty());
}

#[tokio::test]
async fn other_sections_and_comments_stay_byte_identical() {
    let h = Harness::new().await;
    let original = "# my notes\n[work]\naws_access_key_id = X\n\n[other] ; c\nregion = us-west-2\n";
    h.write_credentials(original);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "dev-profile"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let expected = format!(
        "{original}[dev-profile]\naws_access_key_id = ASIAFAKE0001\naws_secret_access_key = fake-secret-1\naws_session_token = fake-token-1\nregion = eu-west-1\n"
    );
    assert_eq!(h.credentials_text().unwrap(), expected);

    h.leapp.stop(id).await.unwrap();
    assert_eq!(h.credentials_text().unwrap(), original);
}

#[cfg(unix)]
#[tokio::test]
async fn creates_aws_folder_and_file_with_private_modes() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new().await;
    assert!(!h.home.path().join(".aws").exists());
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let dir_mode = std::fs::metadata(h.home.path().join(".aws"))
        .unwrap()
        .permissions()
        .mode();
    let file_mode = std::fs::metadata(h.credentials_path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    assert_eq!(file_mode & 0o777, 0o600);
}

#[tokio::test]
async fn token_is_cached_until_it_expires() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();
    h.leapp.stop(id).await.unwrap();
    h.leapp.start(id).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert_eq!(h.credentials_text().as_deref(), Some(DEV_ON_DEFAULT));
    assert!(
        h.secrets
            .snapshot()
            .contains_key(&format!("iam-user-{id}-session-token"))
    );
    assert!(
        h.leapp
            .workspace()
            .session(id)
            .unwrap()
            .session_token_expiration
            .is_some()
    );
}

#[tokio::test]
async fn second_session_on_same_profile_stops_the_first() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    let b = h.leapp.create_iam_user(h.iam_user("b", "default")).unwrap();

    h.leapp.start(a).await.unwrap();
    h.leapp.start(b).await.unwrap();

    let ws = h.leapp.workspace();
    assert_eq!(ws.session(a).unwrap().status, SessionStatus::Inactive);
    assert_eq!(ws.session(b).unwrap().status, SessionStatus::Active);
    let text = h.credentials_text().unwrap();
    assert_eq!(text.matches("[default]").count(), 1);
    assert!(text.contains("ASIAFAKE0002"));
    assert!(!text.contains("ASIAFAKE0001"));
}

#[tokio::test]
async fn sts_error_leaves_file_unchanged() {
    let h = Harness::new().await;
    h.write_credentials("[work]\nx = 1\n");
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    let workspace_before = h.workspace_text();
    h.fake
        .fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key");

    let err = h.leapp.start(id).await.unwrap_err();

    match err {
        Error::Aws(m) => assert!(m.contains("bad key"), "message was: {m}"),
        other => panic!("expected Error::Aws, got {other:?}"),
    }
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(id).unwrap().status, SessionStatus::Inactive);
    assert_eq!(h.credentials_text().as_deref(), Some("[work]\nx = 1\n"));
    assert_eq!(h.workspace_text(), workspace_before);
}

#[tokio::test]
async fn workspace_file_never_contains_temporary_credentials() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let text = h.workspace_text();
    for secret in ["ASIAFAKE0001", "fake-secret-1", "fake-token-1"] {
        assert!(!text.contains(secret), "workspace.json contains {secret}");
    }
}

#[tokio::test]
async fn stop_of_an_inactive_session_does_nothing() {
    let h = Harness::new().await;
    h.write_credentials("[default]\nmine = 1\n");
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.stop(id).await.unwrap();

    assert_eq!(
        h.credentials_text().as_deref(),
        Some("[default]\nmine = 1\n")
    );
}

/// A panic in the middle of a start must not leave the session `Pending`: else every later
/// start, stop, edit or delete of it is refused until the app restarts.
// Multi-thread: the fake AWS server must answer the second start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_that_panics_leaves_the_session_inactive() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    store.panic_on_next_get(&format!("iam-user-{id}-access-key-id"));

    let leapp = &h.leapp;
    let first = std::thread::scope(|scope| scope.spawn(|| block_on(leapp.start(id))).join());

    assert!(first.is_err(), "the first start panics");
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
    h.leapp.start(id).await.unwrap();
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Active
    );
}
