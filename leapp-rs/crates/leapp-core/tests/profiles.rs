//! Settings, profile names, change of region and of named profile (F2, F23-R1, F23-R3, F23-R4).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{FlakyStore, Harness, block_on};
use leapp_core::{Error, Field, SessionId, SessionStatus, Settings};

const MINE: &str = "[default]\naws_access_key_id = MYKEY\naws_secret_access_key = MYSECRET\n";

/// Waits until `done` is true. Panics after 5 s.
async fn settle(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "the condition did not come true");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// An `Active` IAM User session `dev` on `profile` in `region`.
async fn active_session(h: &Harness, profile: &str, region: &str) -> SessionId {
    let mut req = h.iam_user("dev", profile);
    req.region = region.to_string();
    let id = h.leapp.create_iam_user(req).unwrap();
    h.leapp.start(id).await.unwrap();
    id
}

fn status(h: &Harness, id: SessionId) -> SessionStatus {
    h.leapp.workspace().session(id).unwrap().status
}

/// The text of section `[name]` of the credentials file, without its header.
fn section(h: &Harness, name: &str) -> Option<String> {
    let text = h.credentials_text()?;
    let header = format!("[{name}]");
    let mut lines = text.lines().skip_while(|l| l.trim() != header);
    lines.next()?;
    Some(
        lines
            .take_while(|l| !l.trim_start().starts_with('['))
            .map(|l| format!("{l}\n"))
            .collect(),
    )
}

fn invalid(result: Result<impl std::fmt::Debug, Error>) -> (Field, String) {
    match result {
        Err(Error::InvalidInput { field, message }) => (field, message),
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[tokio::test]
async fn change_region_of_an_active_session_rewrites_its_section() {
    let h = Harness::new().await;
    let id = active_session(&h, "default", "us-east-1").await;
    assert!(
        section(&h, "default")
            .unwrap()
            .contains("region = us-east-1")
    );

    h.leapp.change_region(id, "eu-west-1").await.unwrap();

    let body = section(&h, "default").unwrap();
    assert!(body.contains("region = eu-west-1\n"), "{body}");
    assert!(!body.contains("us-east-1"), "{body}");
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "eu-west-1");
    // F1-R1: the restart reuses the cached token.
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    // Saved on disk, not only in memory.
    let h = h.reopen();
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "eu-west-1");
}

#[tokio::test]
async fn change_profile_of_an_active_session_moves_its_section() {
    let h = Harness::new().await;
    let id = active_session(&h, "default", "eu-west-1").await;

    h.leapp.change_profile(id, " dev ").await.unwrap();

    assert_eq!(section(&h, "default"), None, "{:?}", h.credentials_text());
    let body = section(&h, "dev").unwrap();
    for key in [
        "aws_access_key_id",
        "aws_secret_access_key",
        "aws_session_token",
        "region",
    ] {
        assert!(body.contains(&format!("{key} = ")), "{key} in {body}");
    }
    let ws = h.leapp.workspace();
    let dev = ws
        .profiles
        .iter()
        .find(|p| p.name == "dev")
        .expect("profile dev");
    assert_eq!(ws.session(id).unwrap().profile_id, dev.id);
    assert_eq!(status(&h, id), SessionStatus::Active);

    // `default` is allowed as a target.
    h.leapp.change_profile(id, "default").await.unwrap();
    assert_eq!(section(&h, "dev"), None);
    assert!(section(&h, "default").is_some());
    assert_eq!(status(&h, id), SessionStatus::Active);
}

#[tokio::test]
async fn change_profile_checks_the_name() {
    let h = Harness::new().await;
    let id = active_session(&h, "default", "eu-west-1").await;
    let before = h.leapp.workspace();

    assert_eq!(
        invalid(h.leapp.change_profile(id, "a]b").await).0,
        Field::Profile
    );
    // The same message as `create_profile` (T1-m4).
    assert_eq!(
        invalid(h.leapp.change_profile(id, "  ").await),
        (Field::Profile, "Empty profile name".to_string())
    );
    assert_eq!(
        invalid(h.leapp.change_profile(id, "").await),
        (Field::Profile, "Empty profile name".to_string())
    );
    assert_eq!(
        invalid(h.leapp.change_region(id, "mars-1").await).0,
        Field::Region
    );
    assert_eq!(h.leapp.workspace(), before);
}

#[tokio::test]
async fn a_change_that_keeps_the_value_does_nothing() {
    // The system clock: a restart would give a new `start_time`.
    let h = Harness::new().await;
    let id = active_session(&h, "default", "eu-west-1").await;
    let start_time = h.leapp.workspace().session(id).unwrap().start_time;
    let text = h.credentials_text();
    let workspace = h.workspace_text();

    h.leapp.change_region(id, "eu-west-1").await.unwrap();
    h.leapp.change_profile(id, " default ").await.unwrap();

    assert_eq!(
        h.leapp.workspace().session(id).unwrap().start_time,
        start_time
    );
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(h.credentials_text(), text);
    assert_eq!(h.workspace_text(), workspace);
}

#[tokio::test]
async fn a_change_of_a_pending_session_changes_nothing() {
    let h = Harness::new().await;
    let id = h
        .leapp
        .create_iam_user(h.iam_user("dev", "default"))
        .unwrap();
    h.fake
        .delay_next("GetSessionToken", Duration::from_millis(300));

    let (started, changed) = tokio::join!(h.leapp.start(id), async {
        settle(|| status(&h, id) == SessionStatus::Pending).await;
        let region = h.leapp.change_region(id, "us-west-2").await;
        let profile = h.leapp.change_profile(id, "ci").await;
        (region, profile)
    });

    started.unwrap();
    let (region, profile) = changed;
    assert!(
        matches!(region, Err(Error::ProfilePending(_))),
        "{region:?}"
    );
    assert!(
        matches!(profile, Err(Error::ProfilePending(_))),
        "{profile:?}"
    );
    let ws = h.leapp.workspace();
    assert_eq!(ws.session(id).unwrap().region, "eu-west-1");
    assert!(!ws.profiles.iter().any(|p| p.name == "ci"));
    let text = h.workspace_text();
    assert!(!text.contains("us-west-2"), "{text}");
    assert!(!text.contains("\"ci\""), "{text}");
}

/// Review Focus 1: a start between the stop and the save of a change is refused, and the change
/// ends with the session `Active` on the new value.
// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_during_a_change_is_refused() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = active_session(&h, "default", "us-east-1").await;

    // The stop of the change reads the user's backup; that read waits at the gate.
    let gate = store.block_get("aws-profile-backup-");
    let leapp = &h.leapp;
    let (early_start, finished_early, changed) = std::thread::scope(|scope| {
        let changer = scope.spawn(|| block_on(leapp.change_region(id, "eu-west-1")));
        gate.wait_blocked();

        let starter = scope.spawn(|| block_on(leapp.start(id)));
        // A start that is not refused would wait at the gate too (its own stop of the session
        // reads the backup). Wait for it a short time, then open the gate in any case.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !starter.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let finished_early = starter.is_finished();

        gate.open();
        (
            starter.join().unwrap(),
            finished_early,
            changer.join().unwrap(),
        )
    });

    assert!(finished_early, "the start waited for the change");
    assert!(
        matches!(early_start, Err(Error::ProfilePending(_))),
        "{early_start:?}"
    );
    changed.unwrap();
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "eu-west-1");
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert!(
        section(&h, "default")
            .unwrap()
            .contains("region = eu-west-1")
    );
}

#[tokio::test]
async fn profile_names_are_checked() {
    let h = Harness::new().await;

    assert_eq!(
        invalid(h.leapp.create_profile("  ")),
        (Field::Profile, "Empty profile name".to_string())
    );
    assert_eq!(
        invalid(h.leapp.create_profile("default")),
        (
            Field::Profile,
            "\"default\" is not a valid profile name".to_string()
        )
    );
    let dev = h.leapp.create_profile("dev").unwrap();
    assert_eq!(
        invalid(h.leapp.create_profile(" dev ")),
        (Field::Profile, "Profile already exists".to_string())
    );
    assert_eq!(invalid(h.leapp.create_profile("a]b")).0, Field::Profile);
    assert_eq!(invalid(h.leapp.create_profile("a\nb")).0, Field::Profile);

    let ci = h.leapp.create_profile(" ci ").unwrap();
    let h = h.reopen();
    let ws = h.leapp.workspace();
    assert_eq!(ws.profile_name(ci), Some("ci"));
    assert_eq!(ws.profile_name(dev), Some("dev"));
    assert_eq!(ws.profiles.len(), 3, "{:?}", ws.profiles);
}

#[tokio::test]
async fn the_default_region_setting_is_saved() {
    let h = Harness::new().await;
    assert_eq!(h.leapp.workspace().settings.default_region, "us-east-1");

    h.leapp
        .update_settings(Settings {
            default_region: "eu-west-1".into(),
        })
        .unwrap();
    let h = h.reopen();
    assert_eq!(h.leapp.workspace().settings.default_region, "eu-west-1");

    let (field, _) = invalid(h.leapp.update_settings(Settings {
        default_region: "xx-nowhere-1".into(),
    }));
    assert_eq!(field, Field::Region);
    assert_eq!(h.leapp.workspace().settings.default_region, "eu-west-1");
}

/// T1-m1: the save of a change fails after the stop. The session that the change stopped is
/// started again with the old value, and the error of the save is returned.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_whose_save_fails_starts_the_session_again() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials(MINE);
    h.prompter.answer(true);
    let id = active_session(&h, "default", "us-east-1").await;
    let ws_dir = h.home.path().join(".leapp-rs");

    // The stop of the change waits at the delete of the user's backup, after its own save.
    let stop_gate = store.block_delete("aws-profile-backup-default");
    let leapp = &h.leapp;
    let (changed, restarted) = std::thread::scope(|scope| {
        let changer = scope.spawn(|| block_on(leapp.change_region(id, "eu-west-1")));
        stop_gate.wait_blocked();
        // The save of the change fails: the workspace folder is read-only.
        common::set_mode(&ws_dir, 0o500);
        // The restart reads the user's backup (spec 9.3 step 4); it waits here until the folder
        // is writable again, so that its own saves work.
        let start_gate = store.block_get("aws-profile-backup-default");
        stop_gate.open();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !changer.is_finished() && !start_gate.has_waiters() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let restarted = start_gate.has_waiters();
        common::set_mode(&ws_dir, 0o700);
        start_gate.open();
        (changer.join().unwrap(), restarted)
    });

    assert!(matches!(changed, Err(Error::Io(_))), "{changed:?}");
    assert!(restarted, "the change did not start the session again");
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "us-east-1");
    assert!(
        section(&h, "default")
            .unwrap()
            .contains("region = us-east-1")
    );
    let h = h.reopen();
    assert_eq!(h.leapp.workspace().session(id).unwrap().region, "us-east-1");
}

/// The ID of the named profile `name`.
fn profile_id(h: &Harness, name: &str) -> uuid::Uuid {
    h.leapp
        .workspace()
        .profiles
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no profile {name}"))
        .id
}

fn default_cannot_change() -> (Field, String) {
    (
        Field::Profile,
        "the default profile cannot be renamed or deleted".to_string(),
    )
}

#[tokio::test]
async fn rename_of_a_profile_with_an_active_session_moves_its_section() {
    let h = Harness::new().await;
    let id = active_session(&h, "dev", "eu-west-1").await;
    let dev_id = profile_id(&h, "dev");
    let body = section(&h, "dev").unwrap();

    h.leapp.rename_profile(dev_id, "stage").await.unwrap();

    assert_eq!(section(&h, "dev"), None, "{:?}", h.credentials_text());
    // F1-R1: the restart reuses the cached token, so the section moves as it is.
    assert_eq!(section(&h, "stage"), Some(body));
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(profile_id(&h, "stage"), dev_id);
    assert_eq!(h.leapp.workspace().session(id).unwrap().profile_id, dev_id);
    let h = h.reopen();
    assert_eq!(h.leapp.workspace().profile_name(dev_id), Some("stage"));
}

/// Review Focus 2.
#[tokio::test]
async fn rename_to_a_user_written_section_asks_first() {
    const STAGE: &str = "[stage]\naws_access_key_id = MYKEY\naws_secret_access_key = MYSECRET\n";
    let h = Harness::new().await;
    h.write_credentials(STAGE);
    let id = active_session(&h, "dev", "eu-west-1").await;
    let dev_id = profile_id(&h, "dev");
    assert!(section(&h, "dev").is_some());
    h.prompter.answer(false);

    let renamed = h.leapp.rename_profile(dev_id, "stage").await;

    assert!(
        matches!(&renamed, Err(Error::ProfileOverwriteDeclined(p)) if p == "stage"),
        "{renamed:?}"
    );
    assert_eq!(h.prompter.asked(), vec!["stage".to_string()]);
    assert_eq!(h.leapp.workspace().profile_name(dev_id), Some("stage"));
    assert_eq!(status(&h, id), SessionStatus::Inactive);
    // `[dev]` is gone and the user's `[stage]` is byte for byte as they wrote it.
    assert_eq!(h.credentials_text().as_deref(), Some(STAGE));
}

#[tokio::test]
async fn delete_profile_moves_its_sessions_to_default() {
    let h = Harness::new().await;
    let active = active_session(&h, "dev", "eu-west-1").await;
    let other = h.leapp.create_iam_user(h.iam_user("other", "dev")).unwrap();
    let dev_id = profile_id(&h, "dev");
    let default_id = profile_id(&h, "default");
    assert_eq!(h.leapp.workspace().sessions_on(dev_id), 2);
    assert_eq!(h.leapp.workspace().sessions_on(default_id), 0);
    let body = section(&h, "dev").unwrap();

    h.leapp.delete_profile(dev_id).await.unwrap();

    let ws = h.leapp.workspace();
    assert!(!ws.profiles.iter().any(|p| p.name == "dev"), "{ws:?}");
    assert_eq!(ws.session(active).unwrap().profile_id, default_id);
    assert_eq!(ws.session(other).unwrap().profile_id, default_id);
    assert_eq!(ws.sessions_on(default_id), 2);
    assert_eq!(status(&h, active), SessionStatus::Active);
    assert_eq!(status(&h, other), SessionStatus::Inactive);
    assert_eq!(section(&h, "dev"), None);
    assert_eq!(section(&h, "default"), Some(body));
    let h = h.reopen();
    assert!(h.leapp.workspace().profile_name(dev_id).is_none());
}

#[tokio::test]
async fn delete_of_a_profile_without_sessions_only_removes_it() {
    let h = Harness::new().await;
    let dev_id = h.leapp.create_profile("dev").unwrap();
    let ws_before = h.leapp.workspace();

    h.leapp.delete_profile(dev_id).await.unwrap();

    let h = h.reopen();
    let ws = h.leapp.workspace();
    assert!(ws.profile_name(dev_id).is_none());
    assert_eq!(ws.profiles.len(), ws_before.profiles.len() - 1);
}

/// Review Focus 3.
#[tokio::test]
async fn a_moved_session_takes_over_default() {
    let h = Harness::new().await;
    let a = h.leapp.create_iam_user(h.iam_user("a", "default")).unwrap();
    h.leapp.start(a).await.unwrap();
    let b = h.leapp.create_iam_user(h.iam_user("b", "dev")).unwrap();
    h.leapp.start(b).await.unwrap();
    let a_body = section(&h, "default").unwrap();
    let b_body = section(&h, "dev").unwrap();
    assert_ne!(a_body, b_body);

    h.leapp.delete_profile(profile_id(&h, "dev")).await.unwrap();

    assert_eq!(status(&h, b), SessionStatus::Active);
    assert_eq!(status(&h, a), SessionStatus::Inactive);
    assert_eq!(section(&h, "default"), Some(b_body));
    assert_eq!(section(&h, "dev"), None);
}

#[tokio::test]
async fn the_default_profile_cannot_be_renamed_or_deleted() {
    let h = Harness::new().await;
    let default_id = profile_id(&h, "default");
    // The workspace file exists.
    h.leapp.create_profile("dev").unwrap();
    let before = h.workspace_text();

    assert_eq!(
        invalid(h.leapp.rename_profile(default_id, "main").await),
        default_cannot_change()
    );
    assert_eq!(
        invalid(h.leapp.delete_profile(default_id).await),
        default_cannot_change()
    );
    assert_eq!(h.workspace_text(), before);
    assert_eq!(
        h.leapp.workspace().profile_name(default_id),
        Some("default")
    );
}

#[tokio::test]
async fn rename_checks_the_name() {
    // The system clock: a restart would give a new `start_time`.
    let h = Harness::new().await;
    let id = active_session(&h, "dev", "eu-west-1").await;
    h.leapp.create_profile("ci").unwrap();
    let dev_id = profile_id(&h, "dev");
    let start_time = h.leapp.workspace().session(id).unwrap().start_time;
    let text = h.credentials_text();
    let workspace = h.workspace_text();

    let message = |m: &str| (Field::Profile, m.to_string());
    assert_eq!(
        invalid(h.leapp.rename_profile(dev_id, "default").await),
        message("\"default\" is not a valid profile name")
    );
    assert_eq!(
        invalid(h.leapp.rename_profile(dev_id, " ci ").await),
        message("Profile already exists")
    );
    assert_eq!(
        invalid(h.leapp.rename_profile(dev_id, "  ").await),
        message("Empty profile name")
    );
    assert_eq!(
        invalid(h.leapp.rename_profile(dev_id, "a]b").await).0,
        Field::Profile
    );
    // F23-R3: the same name does nothing.
    h.leapp.rename_profile(dev_id, " dev ").await.unwrap();

    assert_eq!(
        h.leapp.workspace().session(id).unwrap().start_time,
        start_time
    );
    assert_eq!(status(&h, id), SessionStatus::Active);
    assert_eq!(h.credentials_text(), text);
    assert_eq!(h.workspace_text(), workspace);
}

#[tokio::test]
async fn rename_or_delete_of_an_unknown_profile_fails() {
    let h = Harness::new().await;
    let id = uuid::Uuid::new_v4();

    let renamed = h.leapp.rename_profile(id, "x").await;
    assert!(
        matches!(renamed, Err(Error::ProfileNotFound(p)) if p == id),
        "{renamed:?}"
    );
    let deleted = h.leapp.delete_profile(id).await;
    assert!(
        matches!(deleted, Err(Error::ProfileNotFound(p)) if p == id),
        "{deleted:?}"
    );
}
