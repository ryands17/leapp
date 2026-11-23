//! Credentials of an IAM User session (spec 11.1, no MFA).

use std::time::Duration;

use aws_sdk_sts::config::timeout::TimeoutConfig;
use aws_sdk_sts::config::{BehaviorVersion, Credentials, Region, retry::RetryConfig};
use aws_sdk_sts::error::{DisplayErrorContext, ProvideErrorMetadata};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Leapp;
use crate::aws_files::SectionBody;
use crate::error::{Error, Result};
use crate::model::Session;
use crate::regions::DEFAULT_REGION;

const TOKEN_DURATION_SECONDS: i32 = 36000;
const STS_TIMEOUT: Duration = Duration::from_secs(10);

/// The cached `GetSessionToken` result. It is stored in the secret store only.
#[derive(Serialize, Deserialize)]
struct CachedToken {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
}

impl Leapp {
    /// Temporary credentials for the session, and when they expire.
    pub(crate) async fn iam_user_credentials(
        &self,
        session: &Session,
    ) -> Result<(SectionBody, DateTime<Utc>)> {
        let secrets = &self.config.secrets;
        let token_key = format!("iam-user-{}-session-token", session.id);
        let region = if session.region.trim().is_empty() {
            DEFAULT_REGION.to_string()
        } else {
            session.region.trim().to_string()
        };

        if let Some(expiration) = session.session_token_expiration
            && expiration > self.config.clock.now()
            && let Some(json) = secrets.get(&token_key)?
            && let Ok(token) = serde_json::from_str::<CachedToken>(&json)
        {
            return Ok((body(token, region), expiration));
        }

        let access_key_id = secrets
            .get(&format!("iam-user-{}-access-key-id", session.id))?
            .ok_or_else(|| Error::Secret("access key ID of the session is missing".into()))?;
        let secret_access_key = secrets
            .get(&format!("iam-user-{}-secret-access-key", session.id))?
            .ok_or_else(|| Error::Secret("secret access key of the session is missing".into()))?;

        let mut config = aws_sdk_sts::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.clone()))
            .credentials_provider(Credentials::new(
                access_key_id.trim(),
                secret_access_key.trim(),
                None,
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
        let output = aws_sdk_sts::Client::from_conf(config.build())
            .get_session_token()
            .duration_seconds(TOKEN_DURATION_SECONDS)
            .send()
            .await
            .map_err(|e| Error::Aws(aws_message(&e)))?;

        let credentials = output
            .credentials()
            .ok_or_else(|| Error::Aws("AWS returned no credentials".into()))?;
        let expiration = credentials.expiration();
        let expiration = DateTime::from_timestamp(expiration.secs(), expiration.subsec_nanos())
            .ok_or_else(|| Error::Aws("AWS returned an invalid expiration time".into()))?;
        let token = CachedToken {
            access_key_id: credentials.access_key_id().trim().to_string(),
            secret_access_key: credentials.secret_access_key().trim().to_string(),
            session_token: credentials.session_token().trim().to_string(),
        };
        secrets.set(&token_key, &serde_json::to_string(&token)?)?;
        Ok((body(token, region), expiration))
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

/// The message from AWS if there is one, else the full error chain.
fn aws_message<E>(error: &aws_sdk_sts::error::SdkError<E>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match error.message() {
        Some(message) => message.to_string(),
        None => DisplayErrorContext(error).to_string(),
    }
}
