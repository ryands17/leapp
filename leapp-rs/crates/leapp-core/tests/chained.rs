mod common;

use common::Harness;
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
