mod common;

use std::sync::Arc;

use common::{FlakyStore, Gate, Harness, block_on};
use jiff::{SignedDuration, Timestamp};
use leapp_core::{
    Error, Field, IamUserUpdate, NewIamRoleChained, SessionId, SessionKind, SessionStatus,
};

/// A change to a valid request.
type Change = dyn Fn(&mut NewIamRoleChained);

const DEPLOY: &str = "arn:aws:iam::123456789012:role/deploy";

fn chained(name: &str, parent: SessionId, profile: &str) -> NewIamRoleChained {
    NewIamRoleChained {
        name: name.to_string(),
        parent_session_id: parent,
        role_arn: DEPLOY.to_string(),
        role_session_name: None,
        external_id: None,
        profile_name: profile.to_string(),
        region: "us-west-2".to_string(),
    }
}

/// An IAM User parent `P` on `default` and a chained session `C` on `ci`.
fn parent_and_child(
    h: &Harness,
    change: impl FnOnce(&mut NewIamRoleChained),
) -> (SessionId, SessionId) {
    let p = h.leapp.create_iam_user(h.iam_user("P", "default")).unwrap();
    let mut req = chained("C", p, "ci");
    change(&mut req);
    let c = h.leapp.create_iam_role_chained(req).unwrap();
    (p, c)
}

fn param<'a>(r: &'a fake_aws::Recorded, name: &str) -> Option<&'a str> {
    r.params.get(name).map(String::as_str)
}

fn assume_role_calls(h: &Harness) -> Vec<fake_aws::Recorded> {
    h.fake
        .requests()
        .into_iter()
        .filter(|r| r.action == "AssumeRole")
        .collect()
}

fn status(h: &Harness, id: SessionId) -> SessionStatus {
    h.leapp.workspace().session(id).unwrap().status
}

#[tokio::test]
async fn a_chained_start_assumes_the_role_with_the_parents_token() {
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |r| r.external_id = Some("ext-1".into()));

    h.leapp.start(c).await.unwrap();

    let actions: Vec<String> = h.fake.requests().into_iter().map(|r| r.action).collect();
    assert_eq!(actions, ["GetSessionToken", "AssumeRole"]);
    let call = &assume_role_calls(&h)[0];
    assert_eq!(call.access_key_id.as_deref(), Some("ASIAFAKE0001"));
    assert_eq!(call.session_token.as_deref(), Some("fake-token-1"));
    assert_eq!(param(call, "RoleArn"), Some(DEPLOY));
    assert_eq!(param(call, "RoleSessionName"), Some("assumed-from-leapp"));
    assert_eq!(param(call, "DurationSeconds"), Some("3600"));
    assert_eq!(param(call, "ExternalId"), Some("ext-1"));

    // The section has the chained keys and the region of the chained session.
    assert_eq!(
        h.credentials_text().as_deref(),
        Some(
            "[ci]\naws_access_key_id = ASIACHAIN0001\naws_secret_access_key = fake-chained-secret-1\naws_session_token = fake-chained-token-1\nregion = us-west-2\n"
        )
    );
    assert_eq!(status(&h, p), SessionStatus::Inactive);
    let ws = h.leapp.workspace();
    let s = ws.session(c).unwrap();
    assert_eq!(s.status, SessionStatus::Active);
    let left = s
        .session_token_expiration
        .expect("the expiration is saved")
        .duration_since(Timestamp::now());
    assert!(
        (SignedDuration::from_secs(3570)..=SignedDuration::from_secs(3610)).contains(&left),
        "{left:?}"
    );
    // A chained session gets no `GetCallerIdentity` call: its account is in the role ARN.
    assert_eq!(h.fake.count("GetCallerIdentity"), 0);
}

#[tokio::test]
async fn a_set_role_session_name_is_sent() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |r| r.role_session_name = Some("me".into()));

    h.leapp.start(c).await.unwrap();

    let call = &assume_role_calls(&h)[0];
    assert_eq!(param(call, "RoleSessionName"), Some("me"));
    assert_eq!(param(call, "ExternalId"), None);
}

#[tokio::test]
async fn a_two_level_chain_works() {
    let h = Harness::new().await;
    let (_, c1) = parent_and_child(&h, |_| {});
    let c2 = h
        .leapp
        .create_iam_role_chained(chained("C2", c1, "ops"))
        .unwrap();

    h.leapp.start(c2).await.unwrap();

    let calls = assume_role_calls(&h);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].access_key_id.as_deref(), Some("ASIAFAKE0001"));
    assert_eq!(calls[1].access_key_id.as_deref(), Some("ASIACHAIN0001"));
    assert_eq!(
        calls[1].session_token.as_deref(),
        Some("fake-chained-token-1")
    );
    assert_eq!(
        h.credentials_text().as_deref(),
        Some(
            "[ops]\naws_access_key_id = ASIACHAIN0002\naws_secret_access_key = fake-chained-secret-2\naws_session_token = fake-chained-token-2\nregion = us-west-2\n"
        )
    );
    assert_eq!(status(&h, c1), SessionStatus::Inactive);
    assert_eq!(status(&h, c2), SessionStatus::Active);
}

#[tokio::test]
async fn a_parent_with_wrong_keys_stops_the_chain() {
    let h = Harness::new().await;
    let original = "[work]\naws_access_key_id = MINE\n";
    h.write_credentials(original);
    let (_, c) = parent_and_child(&h, |_| {});
    h.fake.fail_next(
        "GetSessionToken",
        403,
        "InvalidClientTokenId",
        "The security token included in the request is invalid.",
    );

    let err = h.leapp.start(c).await.unwrap_err();

    match err {
        Error::Aws(m) => assert_eq!(m, "The security token included in the request is invalid."),
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(h.fake.count("AssumeRole"), 0);
    assert_eq!(h.credentials_text().as_deref(), Some(original));
    assert_eq!(status(&h, c), SessionStatus::Inactive);
    assert!(h.leapp.workspace().managed_profiles.is_empty());
}

#[tokio::test]
async fn an_assume_role_error_is_shown() {
    let h = Harness::new().await;
    let original = "[work]\naws_access_key_id = MINE\n";
    h.write_credentials(original);
    let (_, c) = parent_and_child(&h, |_| {});
    h.fake
        .fail_next("AssumeRole", 403, "AccessDenied", "not allowed");

    let err = h.leapp.start(c).await.unwrap_err();

    match err {
        Error::Aws(m) => assert!(m.contains("not allowed"), "{m}"),
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(status(&h, c), SessionStatus::Inactive);
    assert_eq!(h.credentials_text().as_deref(), Some(original));
    assert!(h.leapp.workspace().managed_profiles.is_empty());
}

#[tokio::test]
async fn chained_input_is_checked() {
    let h = Harness::new().await;
    let p = h.leapp.create_iam_user(h.iam_user("P", "default")).unwrap();
    let before = h.workspace_text();
    let cases: Vec<(Field, Box<Change>)> = vec![
        (Field::Name, Box::new(|r| r.name = "  ".into())),
        (
            Field::ParentSession,
            Box::new(|r| r.parent_session_id = SessionId::new_v4()),
        ),
        (Field::RoleArn, Box::new(|r| r.role_arn = "deploy".into())),
        (
            Field::RoleSessionName,
            Box::new(|r| r.role_session_name = Some("x".into())),
        ),
        (
            Field::ExternalId,
            Box::new(|r| r.external_id = Some("a b".into())),
        ),
    ];
    for (field, change) in cases {
        let mut req = chained("C", p, "ci");
        change(&mut req);
        match h.leapp.create_iam_role_chained(req) {
            Err(Error::InvalidInput { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{field}: unexpected result {other:?}"),
        }
        assert_eq!(h.leapp.workspace().sessions.len(), 1, "{field}");
        assert_eq!(h.leapp.workspace().profiles.len(), 1, "{field}");
        assert_eq!(h.workspace_text(), before, "{field}");
    }
}

#[tokio::test]
async fn a_bad_role_arn_gets_the_example_message() {
    let h = Harness::new().await;
    let p = h.leapp.create_iam_user(h.iam_user("P", "default")).unwrap();
    for arn in [
        "deploy",
        "arn:aws:iam::12345678901:role/deploy",
        "arn:aws:iam::123456789012:user/deploy",
        "arn:aws:iam::123456789012:role/",
        "arn:other:iam::123456789012:role/deploy",
    ] {
        let mut req = chained("C", p, "ci");
        req.role_arn = arn.into();
        match h.leapp.create_iam_role_chained(req) {
            Err(Error::InvalidInput {
                field: Field::RoleArn,
                message,
            }) => assert_eq!(
                message, "must look like arn:aws:iam::123456789012:role/name",
                "{arn}"
            ),
            other => panic!("{arn}: unexpected result {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_chained_session_round_trips_in_the_workspace_file() {
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |r| {
        r.role_session_name = Some("me".into());
        r.external_id = Some("ext-1".into());
    });
    let before = h.leapp.workspace().session(c).unwrap().clone();
    assert_eq!(
        before.kind,
        SessionKind::IamRoleChained {
            role_arn: DEPLOY.into(),
            parent_session_id: p,
            role_session_name: Some("me".into()),
            external_id: Some("ext-1".into()),
        }
    );

    let h = h.reopen();

    assert_eq!(h.leapp.workspace().session(c), Some(&before));
    let json: serde_json::Value = serde_json::from_str(&h.workspace_text()).unwrap();
    let saved = json["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == c.to_string())
        .unwrap();
    assert_eq!(saved["kind"]["type"], "iam_role_chained");
}

#[tokio::test]
async fn the_account_comes_from_the_role_arn() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    assert_eq!(
        h.leapp.workspace().session(c).unwrap().account().as_deref(),
        Some("123456789012")
    );

    let p = h
        .leapp
        .create_iam_user(h.iam_user("P2", "default"))
        .unwrap();
    let mut req = chained("CN", p, "cn");
    req.role_arn = "arn:aws-cn:iam::210987654321:role/x".into();
    let cn = h.leapp.create_iam_role_chained(req).unwrap();
    assert_eq!(
        h.leapp
            .workspace()
            .session(cn)
            .unwrap()
            .account()
            .as_deref(),
        Some("210987654321")
    );
}

#[tokio::test]
async fn update_iam_user_refuses_a_chained_session() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    let before = h.workspace_text();

    let err = h
        .leapp
        .update_iam_user(
            c,
            IamUserUpdate {
                name: "renamed".into(),
                profile_name: "ci".into(),
                region: "us-west-2".into(),
                mfa_device: None,
                access_key_id: None,
                secret_access_key: None,
            },
        )
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::WrongSessionType(id) if id == c),
        "{err:?}"
    );
    assert_eq!(h.workspace_text(), before);
}

const DEVICE: &str = "arn:aws:iam::123456789012:mfa/dev";

/// The age of a `GetSessionToken` token from the fake AWS (it uses the real time).
const TOKEN_LIFE: i64 = 36000;

fn secs(n: i64) -> SignedDuration {
    SignedDuration::from_secs(n)
}

/// Like `parent_and_child`, with an MFA device on the parent `P`.
fn mfa_parent_and_child(h: &Harness) -> (SessionId, SessionId) {
    let mut req = h.iam_user("P", "default");
    req.mfa_device = Some(DEVICE.to_string());
    let p = h.leapp.create_iam_user(req).unwrap();
    let c = h
        .leapp
        .create_iam_role_chained(chained("C", p, "ci"))
        .unwrap();
    (p, c)
}

/// The edit of `P` (made by `h.iam_user("P", "default")`) to new keys and `mfa_device`.
fn new_keys_for_p(mfa_device: Option<&str>) -> IamUserUpdate {
    IamUserUpdate {
        name: "P".into(),
        profile_name: "default".into(),
        region: "eu-west-1".into(),
        mfa_device: mfa_device.map(str::to_string),
        access_key_id: Some("AKID-NEW".into()),
        secret_access_key: Some("SECRET-NEW".into()),
    }
}

/// The access key IDs of the `GetSessionToken` calls so far.
fn token_call_keys(h: &Harness) -> Vec<Option<String>> {
    h.fake
        .requests()
        .into_iter()
        .filter(|r| r.action == "GetSessionToken")
        .map(|r| r.access_key_id)
        .collect()
}

fn expiration(h: &Harness, id: SessionId) -> Option<Timestamp> {
    h.leapp
        .workspace()
        .session(id)
        .unwrap()
        .session_token_expiration
}

/// Runs `call` on a new thread. If it has not returned after 10 s (it waits at `gate`), the gate
/// opens so that the test cannot hang, and the test fails.
fn within_time<'scope, T: Send + 'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    gate: &Gate,
    call: impl FnOnce() -> T + Send + 'scope,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    // After a timeout nobody receives: the send error is dropped.
    let _ = scope.spawn(move || {
        let _ = tx.send(call());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => {
            gate.open();
            panic!("the call waited at the gate");
        }
    }
}

#[tokio::test]
async fn the_parents_mfa_is_asked_when_its_token_is_old() {
    let h = Harness::new().await;
    let (_, c) = mfa_parent_and_child(&h);
    h.prompter.mfa_answers(vec![Some("123456")]);

    h.leapp.start(c).await.unwrap();

    assert_eq!(h.prompter.mfa_asked(), ["P"]);
    let requests = h.fake.requests();
    let call = requests
        .iter()
        .find(|r| r.action == "GetSessionToken")
        .unwrap();
    assert_eq!(param(call, "TokenCode"), Some("123456"));
    assert_eq!(param(call, "SerialNumber"), Some(DEVICE));
    assert_eq!(status(&h, c), SessionStatus::Active);
}

#[tokio::test]
async fn the_parents_token_is_reused() {
    let (h, _clock) = Harness::with_manual_clock().await;
    let (p, c) = mfa_parent_and_child(&h);
    h.prompter.mfa_answers(vec![Some("123456")]);
    h.leapp.start(c).await.unwrap();
    h.leapp.stop(c).await.unwrap();

    h.leapp.start(c).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert_eq!(h.fake.count("AssumeRole"), 2);
    assert_eq!(h.prompter.mfa_asked().len(), 1);
    // The second `AssumeRole` is signed with the cached parent token.
    let calls = assume_role_calls(&h);
    assert_eq!(calls[1].session_token.as_deref(), Some("fake-token-1"));
    // The parent's status does not change.
    assert_eq!(status(&h, p), SessionStatus::Inactive);

    // The parent's own start reuses the token too.
    h.leapp.start(p).await.unwrap();
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert_eq!(h.prompter.mfa_asked().len(), 1);
    assert!(h.credentials_text().unwrap().contains("fake-token-1"));
}

#[tokio::test]
async fn a_parent_token_near_expiry_is_renewed() {
    let (h, clock) = Harness::with_manual_clock().await;
    let (_, c) = mfa_parent_and_child(&h);
    h.prompter.mfa_answers(vec![Some("123456"), Some("654321")]);
    h.leapp.start(c).await.unwrap();
    h.leapp.stop(c).await.unwrap();
    // Less than `TOKEN_REUSE_MARGIN` (1200 s) is left.
    clock.advance(secs(TOKEN_LIFE - 1199));

    h.leapp.start(c).await.unwrap();

    assert_eq!(h.prompter.mfa_asked(), ["P", "P"]);
    assert_eq!(h.fake.count("GetSessionToken"), 2);
    let calls = assume_role_calls(&h);
    assert_eq!(calls[1].session_token.as_deref(), Some("fake-token-2"));
}

#[tokio::test]
async fn cancel_of_the_parents_mfa_fails_the_chained_start() {
    let h = Harness::new().await;
    let (_, c) = mfa_parent_and_child(&h);
    h.prompter.mfa_answers(vec![None]);

    let err = h.leapp.start(c).await.unwrap_err();

    assert!(matches!(err, Error::MfaCancelled), "{err:?}");
    assert_eq!(h.fake.count("AssumeRole"), 0);
    assert_eq!(h.fake.count("GetSessionToken"), 0);
    assert_eq!(status(&h, c), SessionStatus::Inactive);
    assert_eq!(h.credentials_text(), None);
}

#[tokio::test]
async fn an_old_chained_session_is_refreshed() {
    let (h, clock) = Harness::with_manual_clock().await;
    let (_, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();
    let started = h.leapp.workspace().session(c).unwrap().start_time.unwrap();
    clock.advance(secs(1201));

    let report = h.leapp.refresh_tick().await;

    assert_eq!(report.refreshed, vec![c], "{report:?}");
    assert_eq!(h.fake.count("AssumeRole"), 2);
    assert_eq!(h.fake.count("GetSessionToken"), 1);
    assert!(
        h.credentials_text()
            .unwrap()
            .contains("aws_session_token = fake-chained-token-2")
    );
    let s = h.leapp.workspace().session(c).unwrap().clone();
    assert_eq!(s.status, SessionStatus::Active);
    assert_eq!(s.start_time, Some(started + secs(1201)));
}

// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chained_start_waits_for_an_edit_of_its_parent() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let (p, c) = parent_and_child(&h, |_| {});

    // The edit writes the new keys of P; the write waits at the gate.
    let gate = store.block_set(&format!("iam-user-{p}-"));
    let leapp = &h.leapp;
    let (early_start, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(p, new_keys_for_p(None))));
        gate.wait_blocked();

        let early_start = within_time(scope, &gate, || block_on(leapp.start(c)));

        gate.open();
        (early_start, editor.join().unwrap())
    });

    assert!(
        matches!(early_start, Err(Error::ProfilePending(_))),
        "{early_start:?}"
    );
    assert_eq!(status(&h, c), SessionStatus::Inactive);
    assert!(h.fake.requests().is_empty());
    edited.unwrap();

    h.leapp.start(c).await.unwrap();
    assert_eq!(token_call_keys(&h), [Some("AKID-NEW".to_string())]);
}

// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_skips_a_chained_session_whose_parent_is_edited() {
    let store = Arc::new(FlakyStore::default());
    let (h, clock) = Harness::with_store_and_manual_clock(store.clone()).await;
    let (p, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();
    clock.advance(secs(1201));
    let before = h.credentials_text();

    let gate = store.block_set(&format!("iam-user-{p}-"));
    let leapp = &h.leapp;
    let (report, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(p, new_keys_for_p(None))));
        gate.wait_blocked();

        let report = within_time(scope, &gate, || block_on(leapp.refresh_tick()));

        gate.open();
        (report, editor.join().unwrap())
    });

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(h.fake.count("AssumeRole"), 1);
    assert_eq!(h.credentials_text(), before);
    assert_eq!(status(&h, c), SessionStatus::Active);
    edited.unwrap();

    // The next tick refreshes it with the new keys of the parent.
    let report = h.leapp.refresh_tick().await;
    assert_eq!(report.refreshed, vec![c], "{report:?}");
    assert_eq!(
        token_call_keys(&h).last().cloned().flatten().as_deref(),
        Some("AKID-NEW")
    );
}

/// A chained start gets a new parent token with the old keys; an edit of the parent's keys (or
/// MFA device) ends before the start saves that token. The token must not be reused after the
/// edit (F1-R4): the next start gets a new one. With `new_device`, the next start asks for the
/// code of the new device.
async fn a_parent_token_from_before_an_edit_is_not_reused(edit: IamUserUpdate, new_device: bool) {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let (p, c) = parent_and_child(&h, |_| {});

    // The chained start has its parent token from AWS; the keychain write of it waits.
    let gate = store.block_set(&format!("iam-user-{p}-session-token"));
    let leapp = &h.leapp;
    let (started, edited) = std::thread::scope(|scope| {
        let starter = scope.spawn(|| block_on(leapp.start(c)));
        gate.wait_blocked();

        let edited = within_time(scope, &gate, || block_on(leapp.update_iam_user(p, edit)));

        gate.open();
        (starter.join().unwrap(), edited)
    });
    edited.unwrap();
    started.unwrap();
    assert_eq!(expiration(&h, p), None);

    h.prompter.mfa_answers(vec![Some("123456")]);
    h.leapp.stop(c).await.unwrap();
    h.leapp.start(c).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 2);
    let calls = assume_role_calls(&h);
    assert_eq!(calls[1].session_token.as_deref(), Some("fake-token-2"));
    let asked: &[&str] = if new_device { &["P"] } else { &[] };
    assert_eq!(h.prompter.mfa_asked(), asked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parent_token_from_before_a_key_edit_is_not_reused() {
    a_parent_token_from_before_an_edit_is_not_reused(new_keys_for_p(None), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parent_token_from_before_an_mfa_edit_is_not_reused() {
    let mut edit = new_keys_for_p(Some(DEVICE));
    edit.access_key_id = None;
    edit.secret_access_key = None;
    a_parent_token_from_before_an_edit_is_not_reused(edit, true).await;
}

/// Changes the `parent_session_id` of `id` in `workspace.json` (a hand edit).
fn set_parent_in_file(h: &Harness, id: SessionId, parent: SessionId) {
    let mut json: serde_json::Value = serde_json::from_str(&h.workspace_text()).unwrap();
    let session = json["sessions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == id.to_string())
        .unwrap();
    session["kind"]["parent_session_id"] = serde_json::Value::String(parent.to_string());
    std::fs::write(h.workspace_path(), serde_json::to_string(&json).unwrap()).unwrap();
}

#[tokio::test]
async fn a_loop_in_the_file_fails_fast() {
    let h = Harness::new().await;
    let p = h.leapp.create_iam_user(h.iam_user("P", "default")).unwrap();
    let c1 = h
        .leapp
        .create_iam_role_chained(chained("C1", p, "ci"))
        .unwrap();
    let c2 = h
        .leapp
        .create_iam_role_chained(chained("C2", c1, "ops"))
        .unwrap();
    // C3 is above the loop: its chain is C3 -> C1 -> C2 -> C1.
    let c3 = h
        .leapp
        .create_iam_role_chained(chained("C3", c1, "qa"))
        .unwrap();
    set_parent_in_file(&h, c1, c2);
    let h = h.reopen();

    // The error names the session that was started.
    for (id, name) in [(c1, "C1"), (c3, "C3")] {
        let err = tokio::time::timeout(std::time::Duration::from_secs(5), h.leapp.start(id))
            .await
            .expect("the start returns at once")
            .unwrap_err();

        assert!(matches!(&err, Error::ChainLoop(n) if n == name), "{err:?}");
        assert_eq!(status(&h, id), SessionStatus::Inactive);
    }
    assert!(h.fake.requests().is_empty());
}

#[tokio::test]
async fn a_missing_parent_in_the_file_fails_the_start() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    set_parent_in_file(&h, c, SessionId::new_v4());
    let h = h.reopen();

    let err = h.leapp.start(c).await.unwrap_err();

    match err {
        Error::InvalidInput {
            field: Field::ParentSession,
            message,
        } => assert_eq!(message, "the parent session of \"C\" is missing"),
        other => panic!("unexpected error {other:?}"),
    }
    assert!(h.fake.requests().is_empty());
    assert_eq!(status(&h, c), SessionStatus::Inactive);
}
