//! A fake AWS STS server for tests.
//!
//! The pinned `aws-sdk-sts` uses the Query protocol: a form-encoded body with
//! `Action=<name>`. This server answers `GetSessionToken` and `GetCallerIdentity` in that protocol.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// One request that the server received.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub action: String,
    pub params: BTreeMap<String, String>,
    /// From the `Credential=` part of the SigV4 `Authorization` header.
    pub access_key_id: Option<String>,
}

struct Failure {
    status: u16,
    code: String,
    message: String,
}

#[derive(Default)]
struct Inner {
    requests: Vec<Recorded>,
    failures: HashMap<String, VecDeque<Failure>>,
    delays: HashMap<String, VecDeque<Duration>>,
    issued: usize,
    account: Option<String>,
}

type Shared = Arc<Mutex<Inner>>;

/// A local HTTP server that acts as AWS STS. It stops when dropped.
pub struct FakeAws {
    url: String,
    inner: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl FakeAws {
    /// Start the server on `127.0.0.1` with a free port.
    pub async fn start() -> FakeAws {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake AWS port");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        let inner = Shared::default();
        let app = Router::new().fallback(handle).with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeAws { url, inner, task }
    }

    /// Base URL, for example `http://127.0.0.1:41234`.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// All requests so far, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.lock().unwrap().requests.clone()
    }

    /// Number of requests so far for `action`.
    pub fn count(&self, action: &str) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.requests.iter().filter(|r| r.action == action).count()
    }

    /// The next call of `action` gets an AWS error. Later calls work again.
    pub fn fail_next(&self, action: &str, status: u16, code: &str, message: &str) {
        let failure = Failure {
            status,
            code: code.to_owned(),
            message: message.to_owned(),
        };
        let mut inner = self.inner.lock().unwrap();
        inner
            .failures
            .entry(action.to_owned())
            .or_default()
            .push_back(failure);
    }

    /// The account number that `GetCallerIdentity` answers. Default `123456789012`.
    pub fn account(&self) -> String {
        account_of(&self.inner.lock().unwrap())
    }

    /// Change the account number that `GetCallerIdentity` answers.
    pub fn set_account(&self, account: &str) {
        self.inner.lock().unwrap().account = Some(account.to_owned());
    }

    /// The next call of `action` waits for `delay` before it gets its answer.
    pub fn delay_next(&self, action: &str, delay: Duration) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .delays
            .entry(action.to_owned())
            .or_default()
            .push_back(delay);
    }
}

impl Drop for FakeAws {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn account_of(inner: &Inner) -> String {
    inner
        .account
        .clone()
        .unwrap_or_else(|| "123456789012".to_owned())
}

async fn handle(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let params = parse_form(&body);
    let action = params.get("Action").cloned().unwrap_or_default();
    let access_key_id = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(access_key_from_authorization);

    let (failure, delay, n, account) = {
        let mut inner = inner.lock().unwrap();
        inner.requests.push(Recorded {
            action: action.clone(),
            params: params.clone(),
            access_key_id,
        });
        let failure = inner
            .failures
            .get_mut(&action)
            .and_then(VecDeque::pop_front);
        let delay = inner.delays.get_mut(&action).and_then(VecDeque::pop_front);
        let n = if failure.is_none() && action == "GetSessionToken" {
            inner.issued += 1;
            inner.issued
        } else {
            0
        };
        (failure, delay, n, account_of(&inner))
    };

    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    if let Some(f) = failure {
        let status = StatusCode::from_u16(f.status).unwrap_or(StatusCode::BAD_REQUEST);
        let xml = format!(
            "<ErrorResponse><Error><Type>Sender</Type><Code>{}</Code><Message>{}</Message></Error><RequestId>fake-err</RequestId></ErrorResponse>",
            f.code, f.message
        );
        return (status, [(header::CONTENT_TYPE, "text/xml")], xml).into_response();
    }
    if action == "GetCallerIdentity" {
        let xml = format!(
            r#"<GetCallerIdentityResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/">
  <GetCallerIdentityResult>
    <Arn>arn:aws:iam::{account}:user/fake</Arn><UserId>AIDAFAKE</UserId><Account>{account}</Account>
  </GetCallerIdentityResult>
  <ResponseMetadata><RequestId>fake-id</RequestId></ResponseMetadata>
</GetCallerIdentityResponse>"#
        );
        return ([(header::CONTENT_TYPE, "text/xml")], xml).into_response();
    }
    if action != "GetSessionToken" {
        let xml = "<ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code><Message>The fake server does not know this action.</Message></Error><RequestId>fake-err</RequestId></ErrorResponse>";
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/xml")],
            xml,
        )
            .into_response();
    }

    let seconds: i64 = params
        .get("DurationSeconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(43200);
    let expiration = (chrono::Utc::now() + chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let xml = format!(
        r#"<GetSessionTokenResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/">
  <GetSessionTokenResult><Credentials>
    <AccessKeyId>ASIAFAKE{n:04}</AccessKeyId><SecretAccessKey>fake-secret-{n}</SecretAccessKey>
    <SessionToken>fake-token-{n}</SessionToken><Expiration>{expiration}</Expiration>
  </Credentials></GetSessionTokenResult>
  <ResponseMetadata><RequestId>fake-{n}</RequestId></ResponseMetadata>
</GetSessionTokenResponse>"#
    );
    ([(header::CONTENT_TYPE, "text/xml")], xml).into_response()
}

/// Read the access key ID from `AWS4-HMAC-SHA256 Credential=<AKID>/...`.
fn access_key_from_authorization(value: &str) -> Option<String> {
    let rest = value.split("Credential=").nth(1)?;
    Some(rest.split('/').next()?.to_owned())
}

fn parse_form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
