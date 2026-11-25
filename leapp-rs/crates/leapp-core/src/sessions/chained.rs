//! IAM Role Chained sessions (spec 10.5, 11.2): a session that assumes a role with the temporary
//! keys of its parent session.

use uuid::Uuid;

use super::iam_user::{Issued, aws_message, expiration_of, session_region};
use crate::aws_files::SectionBody;
use crate::error::{Error, Field, Result};
use crate::model::{
    NewIamRoleChained, Session, SessionId, SessionKind, SessionStatus, role_arn_account,
};
use crate::{Leapp, check_profile_chars, check_region, profile_id_for, required, store};

/// The `DurationSeconds` of each `AssumeRole` call (spec 10.5).
pub const ASSUME_ROLE_DURATION: i32 = 3600;
/// The role session name that is sent when the session has none (spec 10.5).
pub const DEFAULT_ROLE_SESSION_NAME: &str = "assumed-from-leapp";

/// F23-R11: the characters that AWS allows in a role session name.
const ROLE_SESSION_NAME_EXTRA: &str = "+=,.@_-";
/// F23-R11: the characters that AWS allows in an external ID.
const EXTERNAL_ID_EXTRA: &str = "+=,.@:/_-";

impl Leapp {
    /// Adds an `Inactive` IAM Role Chained session. The parent must exist. A new session cannot
    /// be in a loop, so no loop check is needed.
    pub fn create_iam_role_chained(&self, req: NewIamRoleChained) -> Result<SessionId> {
        let mut state = self.state.lock().unwrap();
        let name = required(Field::Name, &req.name)?;
        if state.ws.session(req.parent_session_id).is_none() {
            return Err(Error::InvalidInput {
                field: Field::ParentSession,
                message: "session not found".into(),
            });
        }
        let role_arn = req.role_arn.trim();
        if role_arn_account(role_arn).is_none() {
            return Err(Error::InvalidInput {
                field: Field::RoleArn,
                message: "must look like arn:aws:iam::123456789012:role/name".into(),
            });
        }
        let role_session_name = optional(
            Field::RoleSessionName,
            req.role_session_name.as_deref(),
            64,
            ROLE_SESSION_NAME_EXTRA,
        )?;
        let external_id = optional(
            Field::ExternalId,
            req.external_id.as_deref(),
            1224,
            EXTERNAL_ID_EXTRA,
        )?;
        let profile = required(Field::Profile, &req.profile_name)?;
        check_profile_chars(profile)?;
        check_region(&req.region)?;

        let id = Uuid::new_v4();
        let mut next = state.ws.clone();
        let profile_id = profile_id_for(&mut next, profile);
        next.sessions.push(Session {
            id,
            name: name.to_string(),
            region: req.region.clone(),
            profile_id,
            status: SessionStatus::Inactive,
            start_time: None,
            session_token_expiration: None,
            kind: SessionKind::IamRoleChained {
                role_arn: role_arn.to_string(),
                parent_session_id: req.parent_session_id,
                role_session_name,
                external_id,
            },
        });
        store::save(&self.workspace_path, &next)?;
        state.ws = next;
        Ok(id)
    }

    /// Temporary credentials for any session type. A chained session first gets the credentials
    /// of its parent (the parent's own rules, F23-R7), then calls `AssumeRole` with them in its
    /// own region (spec 10.5). `seen` holds the sessions of the chain so far; a repeat is a loop
    /// (F23-R15).
    pub(crate) async fn session_credentials(
        &self,
        session: &Session,
        seen: &mut Vec<SessionId>,
    ) -> Result<Issued> {
        if seen.contains(&session.id) {
            // `start` and `refresh_session` find a loop before they call this; this check makes
            // sure that a loop never recurses without end. The error names the session that
            // was started (F23-R15).
            let started = seen.first().copied().unwrap_or(session.id);
            let name = self
                .state
                .lock()
                .unwrap()
                .ws
                .session(started)
                .map_or_else(|| session.name.clone(), |s| s.name.clone());
            return Err(Error::ChainLoop(name));
        }
        seen.push(session.id);
        let SessionKind::IamRoleChained {
            role_arn,
            parent_session_id,
            role_session_name,
            external_id,
        } = &session.kind
        else {
            return self.iam_user_credentials(session).await;
        };

        // The key generation is taken with the parent, before its keys are read (F23-R7).
        let (parent, parent_key_generation) = {
            let state = self.state.lock().unwrap();
            let parent = state
                .ws
                .session(*parent_session_id)
                .cloned()
                .ok_or_else(|| missing_parent(session))?;
            let generation = super::key_generation(&state, parent.id);
            (parent, generation)
        };
        // No lock is held here: the parent may ask for an MFA code.
        let parent_issued = Box::pin(self.session_credentials(&parent, seen)).await?;
        // A new parent token is saved on the parent, so that the next chained start and the
        // parent's own start reuse it (F23-R7). A chained parent has no token cache (F23-R8).
        if parent_issued.fresh {
            self.save_parent_token(parent.id, parent_issued.expiration, parent_key_generation);
        }
        let parent_keys = parent_issued.body;

        let region = session_region(session);
        let client = self.sts_client(
            &parent_keys.access_key_id,
            &parent_keys.secret_access_key,
            Some(parent_keys.session_token.clone()),
            &region,
        );
        // After `shutdown` began, no new call to AWS.
        if self.state.lock().unwrap().closing {
            return Err(Error::Closing);
        }
        let call = client
            .assume_role()
            .role_arn(role_arn)
            .role_session_name(
                role_session_name
                    .as_deref()
                    .unwrap_or(DEFAULT_ROLE_SESSION_NAME),
            )
            .duration_seconds(ASSUME_ROLE_DURATION)
            .set_external_id(external_id.clone());
        let output = call.send().await.map_err(|e| Error::Aws(aws_message(&e)))?;

        let credentials = output
            .credentials()
            .ok_or_else(|| Error::Aws("AWS returned no credentials".into()))?;
        Ok(Issued {
            body: SectionBody {
                access_key_id: credentials.access_key_id().trim().to_string(),
                secret_access_key: credentials.secret_access_key().trim().to_string(),
                session_token: credentials.session_token().trim().to_string(),
                region,
            },
            expiration: expiration_of(credentials.expiration())?,
            fresh: false,
        })
    }
}

/// The error of a chained session whose parent is not in the workspace (only a hand edit of
/// `workspace.json` can do that).
pub(super) fn missing_parent(session: &Session) -> Error {
    Error::InvalidInput {
        field: Field::ParentSession,
        message: format!("the parent session of \"{}\" is missing", session.name),
    }
}

/// F23-R11: an optional field is not set when it is empty after trim; else it has 2 to `max`
/// characters, each an ASCII letter, a digit or one of `extra`.
fn optional(field: Field, value: Option<&str>, max: usize, extra: &str) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let chars_ok = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || extra.contains(c));
    if !chars_ok || !(2..=max).contains(&value.len()) {
        return Err(Error::InvalidInput {
            field,
            message: format!("must be 2 to {max} characters of A-Z, a-z, 0-9 and {extra}"),
        });
    }
    Ok(Some(value.to_string()))
}
