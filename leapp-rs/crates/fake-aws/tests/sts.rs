use fake_aws::FakeAws;

fn client(fake: &FakeAws, akid: &str) -> aws_sdk_sts::Client {
    let conf = aws_sdk_sts::Config::builder()
        .behavior_version(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_sts::config::Region::new("eu-west-1"))
        .endpoint_url(fake.url())
        .credentials_provider(aws_sdk_sts::config::Credentials::new(
            akid, "secret", None, None, "test",
        ))
        .build();
    aws_sdk_sts::Client::from_conf(conf)
}

#[tokio::test]
async fn get_session_token_returns_credentials_and_records_the_call() {
    let fake = FakeAws::start().await;
    let out = client(&fake, "AKIDLONG")
        .get_session_token()
        .duration_seconds(36000)
        .send()
        .await
        .unwrap();
    let c = out.credentials().unwrap();
    assert!(c.access_key_id().starts_with("ASIAFAKE"));
    assert_eq!(fake.count("GetSessionToken"), 1);
    let r = &fake.requests()[0];
    assert_eq!(
        r.params.get("DurationSeconds").map(String::as_str),
        Some("36000")
    );
    assert_eq!(r.access_key_id.as_deref(), Some("AKIDLONG"));
}

#[tokio::test]
async fn fail_next_returns_an_aws_error_once() {
    let fake = FakeAws::start().await;
    fake.fail_next(
        "GetSessionToken",
        403,
        "InvalidClientTokenId",
        "The security token included in the request is invalid.",
    );
    let sts = client(&fake, "AKIDLONG");
    let err = sts.get_session_token().send().await.unwrap_err();
    let text = aws_sdk_sts::error::DisplayErrorContext(&err).to_string();
    assert!(text.contains("InvalidClientTokenId"), "{text}");
    assert!(sts.get_session_token().send().await.is_ok());
}
