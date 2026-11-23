mod common;

use std::time::Duration;

use common::Harness;
use leapp_core::{Error, SessionStatus};

const MINE: &str = "[default]\naws_access_key_id = AKIAUSER\naws_secret_access_key = usersecret\n";
const BACKUP_KEY: &str = "aws-profile-backup-default";

#[tokio::test]
async fn declined_overwrite_fails_and_keeps_the_file() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(false);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    let err = h.leapp.start(id).await.unwrap_err();

    assert!(matches!(&err, Error::ProfileOverwriteDeclined(p) if p == "default"));
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
    assert_eq!(h.prompter.asked(), vec!["default".to_string()]);
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert_eq!(h.fake.count("GetSessionToken"), 0);
    assert!(!h.secrets.snapshot().contains_key(BACKUP_KEY));
}

#[tokio::test]
async fn accepted_overwrite_saves_the_original_and_restores_it_on_stop() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let text = h.credentials_text().unwrap();
    assert!(text.contains("ASIAFAKE0001"));
    assert!(!text.contains("AKIAUSER"));
    assert_eq!(
        h.secrets.snapshot().get(BACKUP_KEY).map(String::as_str),
        Some(MINE)
    );
    assert!(h.leapp.workspace().managed_profiles["default"].had_original);
    assert!(!h.workspace_text().contains("usersecret"));

    h.leapp.stop(id).await.unwrap();

    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(!h.secrets.snapshot().contains_key(BACKUP_KEY));
    assert!(h.leapp.workspace().managed_profiles.is_empty());
}

#[tokio::test]
async fn failed_start_after_accepted_overwrite_removes_the_backup() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key");

    let err = h.leapp.start(id).await.unwrap_err();

    assert!(err.to_string().contains("bad key"));
    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(!h.secrets.snapshot().contains_key(BACKUP_KEY));
    assert!(h.leapp.workspace().managed_profiles.is_empty());
}

#[tokio::test]
async fn own_managed_profile_does_not_ask_again() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    let b = h.leapp.create_iam_user(h.iam_user("b", "default")).unwrap();

    h.leapp.start(a).await.unwrap();
    h.leapp.start(b).await.unwrap();

    assert!(h.prompter.asked().is_empty());
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(a).unwrap().status, SessionStatus::Inactive);
    assert_eq!(ws.session(b).unwrap().status, SessionStatus::Active);
}

#[tokio::test]
async fn duplicate_user_section_blocks_start() {
    let h = Harness::new().await;
    let original = "[default]\na = 1\n[default]\nb = 2\n";
    h.write_credentials(original);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    let err = h.leapp.start(id).await.unwrap_err();

    assert!(matches!(&err, Error::DuplicateProfileSection(p) if p == "default"));
    assert_eq!(h.credentials_text().as_deref(), Some(original));
    assert!(h.prompter.asked().is_empty());
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
}

#[tokio::test]
async fn keeps_crlf_and_missing_final_newline() {
    let h = Harness::new().await;
    h.write_credentials("[work]\r\nx = 1");
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();

    h.leapp.start(id).await.unwrap();

    let text = h.credentials_text().unwrap();
    assert!(text.starts_with("[work]\r\nx = 1\r\n[default]\r\n"));
    assert!(!text.replace("\r\n", "").contains('\n'));

    h.leapp.stop(id).await.unwrap();
    assert_eq!(h.credentials_text().as_deref(), Some("[work]\r\nx = 1\r\n"));
}

#[tokio::test]
async fn second_start_on_a_pending_profile_is_refused() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    let b = h.leapp.create_iam_user(h.iam_user("b", "default")).unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (ra, rb) = tokio::join!(h.leapp.start(a), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.leapp.start(b).await
    });

    ra.unwrap();
    assert!(matches!(rb, Err(Error::ProfilePending(_))));
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(a).unwrap().status, SessionStatus::Active);
    assert_eq!(ws.session(b).unwrap().status, SessionStatus::Inactive);
}

#[tokio::test]
async fn stop_of_a_pending_session_is_refused_and_changes_nothing() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (ra, stopped) = tokio::join!(h.leapp.start(a), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let result = h.leapp.stop(a).await;
        let status = h.leapp.workspace().session(a).unwrap().status;
        (result, status)
    });

    ra.unwrap();
    let (result, status_during) = stopped;
    assert!(matches!(result, Err(Error::ProfilePending(_))));
    assert_eq!(status_during, SessionStatus::Pending);
    assert_eq!(
        h.leapp.workspace().session(a).unwrap().status,
        SessionStatus::Active
    );
    assert!(h.credentials_text().unwrap().contains("[default]"));
}

#[tokio::test]
async fn startup_after_crash_removes_our_sections() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    let h = h.reopen();
    h.leapp.startup().await.unwrap();

    assert!(!h.credentials_text().unwrap().contains("[default]"));
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(id).unwrap().status, SessionStatus::Inactive);
    assert!(ws.session(id).unwrap().start_time.is_none());
    assert!(ws.managed_profiles.is_empty());
}

#[tokio::test]
async fn startup_after_crash_restores_the_user_section() {
    let h = Harness::new().await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();

    let h = h.reopen();
    h.leapp.startup().await.unwrap();

    assert_eq!(h.credentials_text().as_deref(), Some(MINE));
    assert!(!h.secrets.snapshot().contains_key(BACKUP_KEY));
}

#[tokio::test]
async fn startup_resets_a_pending_status_saved_by_a_crash() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.leapp.start(id).await.unwrap();
    // Simulate a crash while Pending: the saved file says "pending".
    let text = h.workspace_text().replace("\"active\"", "\"pending\"");
    std::fs::write(h.workspace_path(), text).unwrap();

    let h = h.reopen();
    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Pending
    );
    h.leapp.startup().await.unwrap();

    assert_eq!(
        h.leapp.workspace().session(id).unwrap().status,
        SessionStatus::Inactive
    );
}

#[tokio::test]
async fn shutdown_cleans_all_active_sessions() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    let b = h.leapp.create_iam_user(h.iam_user("b", "ci")).unwrap();
    h.leapp.start(a).await.unwrap();
    h.leapp.start(b).await.unwrap();

    h.leapp.shutdown().await.unwrap();

    let text = h.credentials_text().unwrap();
    assert!(!text.contains("[default]") && !text.contains("[ci]"));
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(a).unwrap().status, SessionStatus::Inactive);
    assert_eq!(ws.session(b).unwrap().status, SessionStatus::Inactive);
}
