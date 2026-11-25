//! Credentials of an IAM User session (spec 11.1, no MFA).

use std::time::Duration;

use aws_sdk_sts::config::timeout::TimeoutConfig;
use aws_sdk_sts::config::{BehaviorVersion, Credentials, Region, retry::RetryConfig};
use aws_sdk_sts::error::{DisplayErrorContext, ProvideErrorMetadata};
use aws_sdk_sts::primitives::DateTime;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Leapp;
use crate::aws_files::SectionBody;
use crate::error::{Error, Result};
use crate::model::{Session, SessionId, SessionKind};
use crate::regions::DEFAULT_REGION;

const TOKEN_DURATION_SECONDS: i32 = 36000;
const STS_TIMEOUT: Duration = Duration::from_secs(10);

/// A cached token is reused only while it has more than this left (F1-R1). It equals the refresh
/// age, so the credentials in the file stay valid until the next refresh.
pub const TOKEN_REUSE_MARGIN: SignedDuration = SignedDuration::from_secs(1200);

/// Temporary credentials for a session.
pub(crate) struct Issued {
    pub body: SectionBody,
    pub expiration: Timestamp,
    /// `true` if this call got a new token from `GetSessionToken`, so the caller may fetch the
    /// account with it. Always `false` for a chained session: its account is in the role ARN.
    pub fresh: bool,
}

/// The cached `GetSessionToken` result. It is stored in the secret store only.
#[derive(Serialize, Deserialize)]
struct CachedToken {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
}

impl Leapp {
    /// Temporary credentials for the session, and when they expire.
    pub(crate) async fn iam_user_credentials(&self, session: &Session) -> Result<Issued> {
        let secrets = &self.config.secrets;
        let token_key = format!("iam-user-{}-session-token", session.id);
        let region = session_region(session);

        if let Some(expiration) = session.session_token_expiration
            && expiration.duration_since(self.config.clock.now()) > TOKEN_REUSE_MARGIN
            && let Some(json) = secrets.get(&token_key)?
            && let Ok(token) = serde_json::from_str::<CachedToken>(&json)
        {
            return Ok(Issued {
                body: body(token, region),
                expiration,
                fresh: false,
            });
        }

        let access_key_id = secrets
            .get(&format!("iam-user-{}-access-key-id", session.id))?
            .ok_or_else(|| Error::Secret("access key ID of the session is missing".into()))?;
        let secret_access_key = secrets
            .get(&format!("iam-user-{}-secret-access-key", session.id))?
            .ok_or_else(|| Error::Secret("secret access key of the session is missing".into()))?;

        let mfa = match &session.kind {
            SessionKind::IamUser { mfa_device, .. } => mfa_device.clone(),
            SessionKind::IamRoleChained { .. } => return Err(Error::WrongSessionType(session.id)),
        };
        // No lock is held here: the prompter may wait for the user.
        let token_code = match mfa {
            Some(serial) => {
                // After `shutdown` began, no question: the app is closing.
                if self.state.lock().unwrap().closing {
                    return Err(Error::Closing);
                }
                let code = self
                    .config
                    .prompter
                    .ask_mfa_code(&session.name)
                    .await
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .ok_or(Error::MfaCancelled)?;
                Some((serial, code))
            }
            None => None,
        };

        let client = self.sts_client(&access_key_id, &secret_access_key, None, &region);
        // After `shutdown` began, no new call to AWS.
        if self.state.lock().unwrap().closing {
            return Err(Error::Closing);
        }
        let mut call = client
            .get_session_token()
            .duration_seconds(TOKEN_DURATION_SECONDS);
        if let Some((serial, code)) = token_code {
            call = call.serial_number(serial).token_code(code);
        }
        let output = call.send().await.map_err(|e| Error::Aws(aws_message(&e)))?;

        let credentials = output
            .credentials()
            .ok_or_else(|| Error::Aws("AWS returned no credentials".into()))?;
        let expiration = expiration_of(credentials.expiration())?;
        let token = CachedToken {
            access_key_id: credentials.access_key_id().trim().to_string(),
            secret_access_key: credentials.secret_access_key().trim().to_string(),
            session_token: credentials.session_token().trim().to_string(),
        };
        secrets.set(&token_key, &serde_json::to_string(&token)?)?;
        Ok(Issued {
            body: body(token, region),
            expiration,
            fresh: true,
        })
    }

    /// An STS client with fixed credentials, the session region, one try and a timeout.
    pub(super) fn sts_client(
        &self,
        access_key_id: &str,
        secret_access_key: &str,
        session_token: Option<String>,
        region: &str,
    ) -> aws_sdk_sts::Client {
        let mut config = aws_sdk_sts::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .credentials_provider(Credentials::new(
                access_key_id.trim(),
                secret_access_key.trim(),
                session_token,
                None,
                "leapp-rs",
            ))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(STS_TIMEOUT)
                    .build(),
            )
            .retry_config(RetryConfig::standard().with_max_attempts(1));
        if let Some(url) = &self.config.endpoints.sts {
            config = config.endpoint_url(url);
        }
        aws_sdk_sts::Client::from_conf(config.build())
    }

    /// Asks AWS for the account number with the temporary credentials and saves it, if the
    /// session still has none and its keys did not change since the start (`key_generation`).
    /// Any failure is dropped: the start already succeeded (F1-R5). No lock is held during the
    /// call.
    pub(crate) async fn store_account(
        &self,
        id: SessionId,
        temp: &SectionBody,
        key_generation: u64,
    ) {
        if self.state.lock().unwrap().closing {
            return;
        }
        let client = self.sts_client(
            &temp.access_key_id,
            &temp.secret_access_key,
            Some(temp.session_token.clone()),
            &temp.region,
        );
        let Ok(output) = client.get_caller_identity().send().await else {
            return;
        };
        let Some(account) = output.account().map(|a| a.trim().to_string()) else {
            return;
        };
        if account.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.closing || super::key_generation(&state, id) != key_generation {
            return;
        }
        let mut next = state.ws.clone();
        let Some(session) = next.sessions.iter_mut().find(|s| s.id == id) else {
            return;
        };
        let SessionKind::IamUser { account: slot, .. } = &mut session.kind else {
            return;
        };
        if slot.is_some() {
            return;
        }
        *slot = Some(account);
        if crate::store::save(&self.workspace_path, &next).is_ok() {
            state.ws = next;
        }
    }

    /// Saves the expiration of a new token of `parent` that a chained start got (F23-R7), so
    /// that the cached token in the keychain is reused. It saves only if the parent's keys did
    /// not change since the caller took `key_generation` (before it read the keys): a token of
    /// the old keys must not be reused with the new ones (F1-R4). Any failure is dropped: the
    /// next start gets a new token (F1-R5). The status and profile of the parent do not change.
    pub(crate) fn save_parent_token(
        &self,
        parent: SessionId,
        expiration: Timestamp,
        key_generation: u64,
    ) {
        let mut state = self.state.lock().unwrap();
        if state.closing || super::key_generation(&state, parent) != key_generation {
            return;
        }
        let mut next = state.ws.clone();
        let Some(session) = next.sessions.iter_mut().find(|s| s.id == parent) else {
            return;
        };
        if !matches!(session.kind, SessionKind::IamUser { .. }) {
            return;
        }
        session.session_token_expiration = Some(expiration);
        if crate::store::save(&self.workspace_path, &next).is_ok() {
            state.ws = next;
        }
    }
}

fn body(token: CachedToken, region: String) -> SectionBody {
    SectionBody {
        access_key_id: token.access_key_id,
        secret_access_key: token.secret_access_key,
        session_token: token.session_token,
        region,
    }
}

/// The region of the session, or `DEFAULT_REGION` if it is empty.
pub(super) fn session_region(session: &Session) -> String {
    match session.region.trim() {
        "" => DEFAULT_REGION.to_string(),
        region => region.to_string(),
    }
}

/// The STS expiration time as a `Timestamp`.
pub(super) fn expiration_of(expiration: &DateTime) -> Result<Timestamp> {
    let invalid = || Error::Aws("AWS returned an invalid expiration time".into());
    let nanos = i32::try_from(expiration.subsec_nanos()).map_err(|_| invalid())?;
    Timestamp::new(expiration.secs(), nanos).map_err(|_| invalid())
}

/// The message from AWS if there is one and it is not empty, else the full error chain.
pub(super) fn aws_message<E>(error: &aws_sdk_sts::error::SdkError<E>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match error.message().map(str::trim) {
        Some(message) if !message.is_empty() => message.to_string(),
        _ => DisplayErrorContext(error).to_string(),
    }
}
