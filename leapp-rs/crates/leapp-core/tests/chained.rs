mod common;

use std::sync::Arc;

use common::{FlakyStore, Gate, Harness, block_on};
use jiff::{SignedDuration, Timestamp};
use leapp_core::{
    Error, Field, IamRoleChainedUpdate, IamUserUpdate, NewIamRoleChained, SessionId, SessionKind,
    SessionStatus,
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
async fn the_wrong_update_function_is_refused() {
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |_| {});
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

    let mut req = same_as_now(&h, c);
    req.name = "renamed".into();
    let err = h.leapp.update_iam_role_chained(p, req).await.unwrap_err();
    assert!(
        matches!(err, Error::WrongSessionType(id) if id == p),
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

/// Starts the chained session `c` and, while the `GetSessionToken` answer of its parent waits
/// 3 s at the fake AWS, runs `during` on another thread. So the start has read the parent's keys
/// and key generation before `during`, and gets its parent token after `during`. The keychain
/// write of that token cannot be the gate: it is under the state lock, or does not occur. Fails
/// if the start fails, or if it ended before `during` (no race was made).
fn start_while_the_parent_token_waits(h: &Harness, c: SessionId, during: impl FnOnce() + Send) {
    h.fake
        .delay_next("GetSessionToken", std::time::Duration::from_secs(3));
    let leapp = &h.leapp;
    let fake = &h.fake;
    let (started, during_ended_first) = std::thread::scope(|scope| {
        let starter = scope.spawn(|| block_on(leapp.start(c)));
        let sent = std::time::Instant::now();
        while fake.count("GetSessionToken") == 0 {
            assert!(sent.elapsed().as_secs() < 5, "no GetSessionToken came");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        scope.spawn(during).join().unwrap();
        let during_ended_first = !starter.is_finished();
        (starter.join().unwrap(), during_ended_first)
    });
    assert!(
        during_ended_first,
        "the chained start ended first: the test did not make the race"
    );
    started.unwrap();
}

/// A chained start gets a new parent token with the old keys; an edit of the parent's keys (or
/// MFA device) ends before the start keeps that token. The token must not be reused after the
/// edit (F1-R4): the next start gets a new one. With `new_device`, the next start asks for the
/// code of the new device.
async fn a_parent_token_from_before_an_edit_is_not_reused(edit: IamUserUpdate, new_device: bool) {
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |_| {});

    start_while_the_parent_token_waits(&h, c, || {
        block_on(h.leapp.update_iam_user(p, edit)).unwrap();
    });
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

// Multi-thread: the fake AWS server must answer while this thread waits on the scoped threads.
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

/// A chained start gets a parent token with the old keys. Before it can keep that token, an edit
/// of the parent ends and the parent's own start keeps a token of the new keys. The late token of
/// the old keys must not replace it: the next start of the parent reuses the new-keys token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_parent_token_from_before_an_edit_does_not_replace_a_newer_token() {
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |_| {});

    start_while_the_parent_token_waits(&h, c, || {
        block_on(h.leapp.update_iam_user(p, new_keys_for_p(None))).unwrap();
        block_on(h.leapp.start(p)).unwrap();
    });
    assert_eq!(
        token_call_keys(&h),
        [Some("AKID-P".to_string()), Some("AKID-NEW".to_string())]
    );

    // The next start of the parent reuses the token of the new keys.
    h.leapp.stop(p).await.unwrap();
    h.leapp.start(p).await.unwrap();

    assert_eq!(h.fake.count("GetSessionToken"), 2);
    let text = h.credentials_text().unwrap();
    assert!(
        text.contains("aws_session_token = fake-token-2"),
        "the parent uses a token of its old keys"
    );
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

// ---- Edit of chained sessions (F23-R14) and the delete cascade (F23-R6) ----

const OTHER_ROLE: &str = "arn:aws:iam::210987654321:role/other";

/// An update that keeps every value of the chained session `id` as it is now.
fn same_as_now(h: &Harness, id: SessionId) -> IamRoleChainedUpdate {
    let ws = h.leapp.workspace();
    let s = ws.session(id).unwrap();
    let SessionKind::IamRoleChained {
        role_arn,
        parent_session_id,
        role_session_name,
        external_id,
    } = &s.kind
    else {
        panic!("not a chained session");
    };
    IamRoleChainedUpdate {
        name: s.name.clone(),
        parent_session_id: *parent_session_id,
        role_arn: role_arn.clone(),
        role_session_name: role_session_name.clone(),
        external_id: external_id.clone(),
        profile_name: ws.profile_name(s.profile_id).unwrap().to_string(),
        region: s.region.clone(),
    }
}

/// The keychain keys of the IAM User session `id`.
fn keys_of(h: &Harness, id: SessionId) -> Vec<String> {
    let prefix = format!("iam-user-{id}-");
    h.secrets
        .snapshot()
        .into_keys()
        .filter(|k| k.starts_with(&prefix))
        .collect()
}

/// Waits until `done` is true. Panics after 5 s.
async fn settle(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "the condition did not come true"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn edit_of_a_chained_session_saves_its_fields() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    let p2 = h.leapp.create_iam_user(h.iam_user("P2", "other")).unwrap();

    h.leapp
        .update_iam_role_chained(
            c,
            IamRoleChainedUpdate {
                name: " C renamed ".into(),
                parent_session_id: p2,
                role_arn: format!(" {OTHER_ROLE} "),
                role_session_name: Some(" me ".into()),
                external_id: Some("ext-2".into()),
                profile_name: "ops".into(),
                region: "eu-central-1".into(),
            },
        )
        .await
        .unwrap();

    // Saved in the file: a new `Leapp` reads the new values.
    let h = h.reopen();
    let ws = h.leapp.workspace();
    let s = ws.session(c).unwrap();
    assert_eq!(s.name, "C renamed");
    assert_eq!(s.region, "eu-central-1");
    assert_eq!(ws.profile_name(s.profile_id), Some("ops"));
    assert_eq!(s.status, SessionStatus::Inactive);
    assert_eq!(
        s.kind,
        SessionKind::IamRoleChained {
            role_arn: OTHER_ROLE.into(),
            parent_session_id: p2,
            role_session_name: Some("me".into()),
            external_id: Some("ext-2".into()),
        }
    );
    assert!(h.fake.requests().is_empty());
}

#[tokio::test]
async fn edit_of_an_active_chained_session_restarts_it() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();

    let mut req = same_as_now(&h, c);
    req.role_arn = OTHER_ROLE.into();
    h.leapp.update_iam_role_chained(c, req).await.unwrap();

    let calls = assume_role_calls(&h);
    assert_eq!(calls.len(), 2);
    assert_eq!(param(&calls[1], "RoleArn"), Some(OTHER_ROLE));
    assert_eq!(status(&h, c), SessionStatus::Active);

    // A name-only edit does not restart.
    let mut req = same_as_now(&h, c);
    req.name = "C renamed".into();
    h.leapp.update_iam_role_chained(c, req).await.unwrap();

    assert_eq!(h.fake.count("AssumeRole"), 2);
    assert_eq!(status(&h, c), SessionStatus::Active);
    assert_eq!(h.leapp.workspace().session(c).unwrap().name, "C renamed");
}

#[tokio::test]
async fn a_change_of_parent_restarts_with_the_new_parent() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    let p2 = h.leapp.create_iam_user(h.iam_user("P2", "other")).unwrap();
    h.leapp.start(c).await.unwrap();

    let mut req = same_as_now(&h, c);
    req.parent_session_id = p2;
    h.leapp.update_iam_role_chained(c, req).await.unwrap();

    assert_eq!(
        token_call_keys(&h),
        [Some("AKID-P".to_string()), Some("AKID-P2".to_string())]
    );
    assert_eq!(h.fake.count("AssumeRole"), 2);
    assert_eq!(status(&h, c), SessionStatus::Active);
}

#[tokio::test]
async fn a_loop_is_rejected_on_edit() {
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
    let c3 = h
        .leapp
        .create_iam_role_chained(chained("C3", c2, "qa"))
        .unwrap();
    let before = h.workspace_text();

    for (parent, expected) in [
        (c2, "would make a loop of chained sessions"),
        (c3, "would make a loop of chained sessions"),
        (c1, "must not be the session itself"),
    ] {
        let mut req = same_as_now(&h, c1);
        req.parent_session_id = parent;
        match h.leapp.update_iam_role_chained(c1, req).await {
            Err(Error::InvalidInput {
                field: Field::ParentSession,
                message,
            }) => assert_eq!(message, expected),
            other => panic!("unexpected result {other:?}"),
        }
        assert_eq!(h.workspace_text(), before);
    }
}

#[tokio::test]
async fn chained_edit_input_is_checked() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    let before = h.workspace_text();
    type UpdateChange = dyn Fn(&mut IamRoleChainedUpdate);
    let cases: Vec<(Field, Box<UpdateChange>)> = vec![
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
        (Field::Profile, Box::new(|r| r.profile_name = "[x]".into())),
        (Field::Region, Box::new(|r| r.region = "mars-1".into())),
    ];
    for (field, change) in cases {
        let mut req = same_as_now(&h, c);
        change(&mut req);
        match h.leapp.update_iam_role_chained(c, req).await {
            Err(Error::InvalidInput { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{field}: unexpected result {other:?}"),
        }
        assert_eq!(h.workspace_text(), before, "{field}");
    }
}

#[tokio::test]
async fn edit_of_a_pending_chained_session_is_refused() {
    let h = Harness::new().await;
    let (_, c) = parent_and_child(&h, |_| {});
    h.fake
        .delay_next("AssumeRole", std::time::Duration::from_millis(300));

    let (started, edited) = tokio::join!(h.leapp.start(c), async {
        settle(|| status(&h, c) == SessionStatus::Pending).await;
        let mut req = same_as_now(&h, c);
        req.role_arn = OTHER_ROLE.into();
        h.leapp.update_iam_role_chained(c, req).await
    });

    started.unwrap();
    assert!(
        matches!(edited, Err(Error::ProfilePending(_))),
        "{edited:?}"
    );
    assert_eq!(param(&assume_role_calls(&h)[0], "RoleArn"), Some(DEPLOY));
    assert_eq!(assume_role_calls(&h).len(), 1);
}

#[tokio::test]
async fn delete_of_a_parent_deletes_its_chain() {
    let h = Harness::new().await;
    let mine = "[ops]\naws_access_key_id = MINE\n";
    h.write_credentials(mine);
    h.prompter.answer(true);
    let (p, c1) = parent_and_child(&h, |_| {});
    let c2 = h
        .leapp
        .create_iam_role_chained(chained("C2", c1, "ops"))
        .unwrap();
    let outside = h.leapp.create_iam_user(h.iam_user("O", "other")).unwrap();
    h.leapp.start(c2).await.unwrap();
    h.leapp.start(p).await.unwrap();
    assert_eq!(keys_of(&h, p).len(), 3);

    h.leapp.delete_session(p).await.unwrap();

    let ws = h.leapp.workspace();
    for id in [p, c1, c2] {
        assert!(ws.session(id).is_none());
        assert!(!h.workspace_text().contains(&id.to_string()));
    }
    assert!(ws.session(outside).is_some());
    assert!(ws.managed_profiles.is_empty());
    assert!(keys_of(&h, p).is_empty(), "{:?}", keys_of(&h, p));
    // No section of the chain is left: the user's own section of `ops` is back.
    assert_eq!(h.credentials_text().as_deref(), Some(mine));
    assert!(!h.secrets.snapshot().contains_key("aws-profile-backup-ops"));
}

#[tokio::test]
async fn delete_of_a_parent_with_a_pending_child_changes_nothing() {
    // The `Pending` session is the child, then the grandchild.
    for pending_depth in [1, 2] {
        let h = Harness::new().await;
        let (p, c1) = parent_and_child(&h, |_| {});
        let c2 = h
            .leapp
            .create_iam_role_chained(chained("C2", c1, "ops"))
            .unwrap();
        let pending = if pending_depth == 1 { c1 } else { c2 };
        h.fake
            .delay_next("AssumeRole", std::time::Duration::from_millis(300));

        let (started, deleted) = tokio::join!(h.leapp.start(pending), async {
            settle(|| status(&h, pending) == SessionStatus::Pending).await;
            h.leapp.delete_session(p).await
        });

        started.unwrap();
        assert!(
            matches!(deleted, Err(Error::ProfilePending(_))),
            "{pending_depth}: {deleted:?}"
        );
        let ws = h.leapp.workspace();
        for id in [p, c1, c2] {
            assert!(ws.session(id).is_some(), "{pending_depth}");
        }
        assert_eq!(keys_of(&h, p).len(), 3, "{pending_depth}");
        assert_eq!(status(&h, pending), SessionStatus::Active);
    }
}

/// An edit of a child holds it (its restart waits at the delete of the user's backup). A delete
/// of the parent must not delete the child under the edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_of_a_parent_while_an_edit_holds_a_child_changes_nothing() {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials("[ci]\naws_access_key_id = MINE\n");
    h.prompter.answer(true);
    let (p, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();

    let gate = store.block_delete("aws-profile-backup-ci");
    let mut req = same_as_now(&h, c);
    req.region = "eu-central-1".into();
    let leapp = &h.leapp;
    let (deleted, edited) = std::thread::scope(|scope| {
        let editor = scope.spawn(|| block_on(leapp.update_iam_role_chained(c, req)));
        gate.wait_blocked();
        let deleted = within_time(scope, &gate, || block_on(leapp.delete_session(p)));
        gate.open();
        (deleted, editor.join().unwrap())
    });

    assert!(
        matches!(deleted, Err(Error::ProfilePending(_))),
        "{deleted:?}"
    );
    edited.unwrap();
    let ws = h.leapp.workspace();
    assert!(ws.session(p).is_some());
    assert_eq!(ws.session(c).unwrap().region, "eu-central-1");
    assert_eq!(status(&h, c), SessionStatus::Active);
    assert_eq!(keys_of(&h, p).len(), 3);
}

#[tokio::test]
async fn descendants_lists_each_session_once() {
    let h = Harness::new().await;
    let p = h.leapp.create_iam_user(h.iam_user("P", "default")).unwrap();
    let a = h
        .leapp
        .create_iam_role_chained(chained("A", p, "ci"))
        .unwrap();
    let b = h
        .leapp
        .create_iam_role_chained(chained("B", p, "ops"))
        .unwrap();
    let g = h
        .leapp
        .create_iam_role_chained(chained("G", a, "qa"))
        .unwrap();
    h.leapp.create_iam_user(h.iam_user("O", "other")).unwrap();

    let ws = h.leapp.workspace();
    assert_eq!(ws.descendants(p), [a, b, g]);
    assert_eq!(ws.descendants(a), [g]);
    assert!(ws.descendants(g).is_empty());
}

/// A loop in `workspace.json` (a hand edit): `descendants` ends, and a delete removes the loop.
#[tokio::test]
async fn a_loop_in_the_file_ends_descendants_and_delete() {
    let h = Harness::new().await;
    let (p, c1) = parent_and_child(&h, |_| {});
    let c2 = h
        .leapp
        .create_iam_role_chained(chained("C2", c1, "ops"))
        .unwrap();
    set_parent_in_file(&h, c1, c2);
    let h = h.reopen();

    let ws = h.leapp.workspace();
    assert_eq!(ws.descendants(c1), [c2]);
    assert_eq!(ws.descendants(c2), [c1]);
    assert!(ws.descendants(p).is_empty());

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        h.leapp.delete_session(c1),
    )
    .await
    .expect("the delete returns at once")
    .unwrap();
    let ws = h.leapp.workspace();
    assert!(ws.session(c1).is_none());
    assert!(ws.session(c2).is_none());
    assert!(ws.session(p).is_some());
}

/// Whether this process can write to a folder of mode 0o500 (root can).
#[cfg(unix)]
fn modes_are_ignored() -> bool {
    let dir = tempfile::TempDir::new().unwrap();
    common::set_mode(dir.path(), 0o500);
    let ignored = std::fs::write(dir.path().join("probe"), "x").is_ok();
    common::set_mode(dir.path(), 0o700);
    ignored
}

/// The stop of an active child fails: the delete returns the error and removes nothing.
#[cfg(unix)]
#[tokio::test]
async fn a_delete_whose_stop_fails_removes_nothing() {
    if modes_are_ignored() {
        eprintln!("skipped: this process ignores folder modes");
        return;
    }
    let h = Harness::new().await;
    let (p, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();
    let before = h.credentials_text();

    common::set_mode(&h.home.path().join(".aws"), 0o500);
    let deleted = h.leapp.delete_session(p).await;
    common::set_mode(&h.home.path().join(".aws"), 0o700);

    assert!(deleted.is_err(), "{deleted:?}");
    let ws = h.leapp.workspace();
    assert!(ws.session(p).is_some());
    assert_eq!(status(&h, c), SessionStatus::Active);
    assert_eq!(keys_of(&h, p).len(), 3);
    assert_eq!(h.credentials_text(), before);
}

/// A delete of `P` holds its chain: the stop of the child `C` waits at the delete of the user's
/// backup of `ci`. Meanwhile, `during` runs. Returns what `during` returned, and the delete must
/// succeed.
fn while_a_delete_holds_the_chain<T: Send>(
    h: &Harness,
    store: &FlakyStore,
    p: SessionId,
    during: impl FnOnce() -> T + Send,
) -> T {
    let gate = store.block_delete("aws-profile-backup-ci");
    let leapp = &h.leapp;
    let (deleted, out) = std::thread::scope(|scope| {
        let deleter = scope.spawn(|| block_on(leapp.delete_session(p)));
        gate.wait_blocked();
        let out = within_time(scope, &gate, during);
        gate.open();
        (deleter.join().unwrap(), out)
    });
    deleted.unwrap();
    out
}

/// An active chain `P` -> `C` (with the user's own `ci` section backed up), and its harness.
async fn active_chain_with_backup() -> (Harness, Arc<FlakyStore>, SessionId, SessionId) {
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    h.write_credentials("[ci]\naws_access_key_id = MINE\n");
    h.prompter.answer(true);
    let (p, c) = parent_and_child(&h, |_| {});
    h.leapp.start(c).await.unwrap();
    (h, store, p, c)
}

/// A create under a session that a delete holds is refused: else the delete removes the new
/// session after the user saw it created.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_under_a_chain_that_a_delete_holds_is_refused() {
    let (h, store, p, c) = active_chain_with_backup().await;

    let created = while_a_delete_holds_the_chain(&h, &store, p, || {
        [p, c].map(|parent| h.leapp.create_iam_role_chained(chained("N", parent, "qa")))
    });

    for result in created {
        assert!(
            matches!(result, Err(Error::ProfilePending(_))),
            "{result:?}"
        );
    }
    assert!(
        h.leapp.workspace().sessions.is_empty(),
        "the delete removed the chain, and no session was created"
    );
}

/// An edit that moves a session under a session that a delete holds is refused and changes
/// nothing: else the delete removes the edited session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_to_a_parent_that_a_delete_holds_is_refused() {
    let (h, store, p, c) = active_chain_with_backup().await;
    let o = h.leapp.create_iam_user(h.iam_user("O", "other")).unwrap();
    let d = h
        .leapp
        .create_iam_role_chained(chained("D", o, "qa"))
        .unwrap();
    let mut req = same_as_now(&h, d);
    req.parent_session_id = c;

    let edited = while_a_delete_holds_the_chain(&h, &store, p, || {
        block_on(h.leapp.update_iam_role_chained(d, req))
    });

    assert!(
        matches!(edited, Err(Error::ProfilePending(_))),
        "{edited:?}"
    );
    let ws = h.leapp.workspace();
    assert!(ws.session(c).is_none(), "the delete removed the chain");
    let SessionKind::IamRoleChained {
        parent_session_id, ..
    } = &ws.session(d).expect("the edited session is kept").kind
    else {
        panic!("not a chained session");
    };
    assert_eq!(*parent_session_id, o, "the edit changed nothing");
}

/// A chained start reads the parent's new keys while an edit of the parent writes them, and the
/// save of that edit then fails: the edit puts the old keys back. The parent token that the start
/// gets with the rejected keys must not be kept or reused (F1-R4).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parent_token_of_keys_from_a_failed_edit_is_not_kept() {
    if modes_are_ignored() {
        eprintln!("skipped: this process ignores folder modes");
        return;
    }
    let store = Arc::new(FlakyStore::default());
    let h = Harness::with_store(store.clone()).await;
    let (p, c) = parent_and_child(&h, |_| {});
    let ws_dir = h.home.path().join(".leapp-rs");
    // The start reads the parent's keys only after the edit wrote the new access key ID. The
    // edit's own read of the old keys does not wait.
    let read_gate = store.block_next_get(&format!("iam-user-{p}-access-key-id"));
    // The edit waits before it writes the new secret key, so before its save. So the start
    // gets its token with the new access key ID, which the edit never saves.
    let write_gate = store.block_set(&format!("iam-user-{p}-secret-access-key"));
    h.fake
        .delay_next("GetSessionToken", std::time::Duration::from_secs(2));

    let leapp = &h.leapp;
    let fake = &h.fake;
    let (started, edited, edit_ended_first) = std::thread::scope(|scope| {
        let starter = scope.spawn(|| block_on(leapp.start(c)));
        read_gate.wait_blocked();
        // The save of the edit fails: the workspace folder is read-only.
        common::set_mode(&ws_dir, 0o500);
        let editor = scope.spawn(|| block_on(leapp.update_iam_user(p, new_keys_for_p(None))));
        write_gate.wait_blocked();
        read_gate.open();
        let sent = std::time::Instant::now();
        while fake.count("GetSessionToken") == 0 {
            if sent.elapsed().as_secs() >= 5 {
                // Open the gate first, so that the edit ends and the test does not hang.
                write_gate.open();
                panic!("no GetSessionToken came");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        write_gate.open();
        let edited = editor.join().unwrap();
        common::set_mode(&ws_dir, 0o700);
        let edit_ended_first = !starter.is_finished();
        (starter.join().unwrap(), edited, edit_ended_first)
    });

    assert!(edit_ended_first, "the start ended first: no race was made");
    assert!(matches!(edited, Err(Error::Io(_))), "{edited:?}");
    assert_eq!(
        token_call_keys(&h),
        [Some("AKID-NEW".to_string())],
        "the start got its token with the rejected key"
    );
    started.unwrap();
    assert_eq!(
        h.secrets
            .snapshot()
            .get(&format!("iam-user-{p}-access-key-id"))
            .map(String::as_str),
        Some("AKID-P"),
        "the failed edit put the old key back"
    );
    assert_eq!(expiration(&h, p), None, "the token got an expiration");
    assert!(
        !h.secrets
            .snapshot()
            .contains_key(&format!("iam-user-{p}-session-token")),
        "the token of the rejected keys is in the keychain"
    );
}
