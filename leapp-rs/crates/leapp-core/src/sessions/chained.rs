//! IAM Role Chained sessions (spec 10.5, 11.2): a session that assumes a role with the temporary
//! keys of its parent session.

use uuid::Uuid;

use super::edit::{Editing, changed_meanwhile};
use super::iam_user::{Issued, aws_message, expiration_of, session_region};
use super::{parent_editing_error, pending_error, pending_on, profile_name};
use crate::aws_files::SectionBody;
use crate::error::{Error, Field, Result};
use crate::model::{
    IamRoleChainedUpdate, NewIamRoleChained, Session, SessionId, SessionKind, SessionStatus,
    role_arn_account,
};
use crate::{Leapp, State, check_profile_chars, check_region, profile_id_for, required, store};

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
    /// be in a loop, so no loop check is needed. A parent that an edit, a change or a delete
    /// holds gives `Error::ProfilePending` and changes nothing: else a delete of the chain can
    /// remove the new session.
    pub fn create_iam_role_chained(&self, req: NewIamRoleChained) -> Result<SessionId> {
        let mut state = self.state.lock().unwrap();
        let valid = validate(&state, &req, None)?;

        let id = Uuid::new_v4();
        let mut next = state.ws.clone();
        let profile_id = profile_id_for(&mut next, valid.profile);
        next.sessions.push(Session {
            id,
            name: valid.name.to_string(),
            region: req.region.clone(),
            profile_id,
            status: SessionStatus::Inactive,
            start_time: None,
            session_token_expiration: None,
            kind: valid.kind,
        });
        store::save(&self.workspace_path, &next)?;
        state.ws = next;
        Ok(id)
    }

    /// Edits an IAM Role Chained session with the validation of `create_iam_role_chained`. The
    /// new parent must not be the session itself (`"must not be the session itself"`) or a
    /// session below it (`"would make a loop of chained sessions"`).
    ///
    /// F23-R14: if the session is `Active` and anything but the name changes (the parent too),
    /// the session is stopped, saved and started again. A name-only edit does not restart. An
    /// edit of a `Pending` session, or of one that another edit or change holds, returns
    /// `Error::ProfilePending` and changes nothing. `update_iam_user` is the edit of an IAM User
    /// session: this function returns `Error::WrongSessionType` for one.
    ///
    /// A new parent that an edit, a change or a delete holds gives `Error::ProfilePending`. The
    /// parent is checked again in the critical section that saves, because a delete or an
    /// edit may change the chain while the session stops. If that check or the save fails after
    /// the stop, the session stays `Inactive` and the error is returned. If the restart fails,
    /// the edit stays saved and the session is `Inactive`; the error of the start is returned.
    /// If the stop finds the user's backup missing, the edit completes and then returns
    /// `Error::BackupMissing` (F1-R8).
    ///
    /// This function does blocking file and keychain I/O. Call it from a task that may block.
    pub async fn update_iam_role_chained(
        &self,
        id: SessionId,
        req: IamRoleChainedUpdate,
    ) -> Result<()> {
        let req = as_new(req);
        let (was_active, restart, reservation) = {
            let mut state = self.state.lock().unwrap();
            let session = state.ws.session(id).ok_or(Error::NotFound(id))?;
            if !matches!(session.kind, SessionKind::IamRoleChained { .. }) {
                return Err(Error::WrongSessionType(id));
            }
            let valid = validate(&state, &req, Some(id))?;
            if session.status == SessionStatus::Pending {
                return Err(pending_error());
            }
            let was_active = session.status == SessionStatus::Active;
            let changed = valid.kind != session.kind
                || req.region != session.region
                || valid.profile != profile_name(&state.ws, session.profile_id)?;
            let restart = was_active && changed;
            // The restart would fail on a profile that another session starts on now.
            let new_profile = state.ws.profiles.iter().find(|p| p.name == valid.profile);
            if restart && new_profile.is_some_and(|p| pending_on(&state.ws, p.id)) {
                return Err(pending_error());
            }
            let reservation = Editing::reserve(&self.state, &mut state, id)?;
            (was_active, restart, reservation)
        };

        let mut backup_missing = None;
        if restart {
            match self.stop(id).await {
                Ok(()) => {}
                Err(Error::BackupMissing(p)) => backup_missing = Some(p),
                Err(e) => return Err(e),
            }
        }

        {
            let mut state = self.state.lock().unwrap();
            let expected = if was_active && !restart {
                SessionStatus::Active
            } else {
                SessionStatus::Inactive
            };
            match state.ws.session(id) {
                None => return Err(Error::NotFound(id)),
                Some(s) if s.status != expected => return Err(changed_meanwhile()),
                Some(_) => {}
            }
            let valid = validate(&state, &req, Some(id))?;
            let mut next = state.ws.clone();
            let profile_id = profile_id_for(&mut next, valid.profile);
            let session = next
                .sessions
                .iter_mut()
                .find(|s| s.id == id)
                .expect("the session was found above");
            session.name = valid.name.to_string();
            session.region = req.region.clone();
            session.profile_id = profile_id;
            session.kind = valid.kind;
            store::save(&self.workspace_path, &next)?;
            state.ws = next;
        }

        // The edit is saved; the restart below is a normal start.
        drop(reservation);
        if restart {
            self.start(id).await?;
        }
        match backup_missing {
            Some(p) => Err(Error::BackupMissing(p)),
            None => Ok(()),
        }
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
        // The first session of `seen` is the one that was started; the others are its parents.
        let own_start = seen.is_empty();
        seen.push(session.id);
        let SessionKind::IamRoleChained {
            role_arn,
            parent_session_id,
            role_session_name,
            external_id,
        } = &session.kind
        else {
            // A parent's new token is kept by `save_parent_token` below, under the key
            // generation check; only the session's own start keeps it at once.
            return self.iam_user_credentials(session, own_start).await;
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
        // A new parent token is kept on the parent, so that the next chained start and the
        // parent's own start reuse it (F23-R7). A chained parent has no token cache (F23-R8).
        if parent_issued.fresh {
            self.save_parent_token(
                parent.id,
                &parent_issued.body,
                parent_issued.expiration,
                parent_key_generation,
            );
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

/// The valid values of a chained session form.
struct Valid<'a> {
    name: &'a str,
    profile: &'a str,
    kind: SessionKind,
}

/// The validation of `create_iam_role_chained` and `update_iam_role_chained`, in the order of the
/// form fields. `edited` is the session of an edit: its new parent must not be the session itself
/// or a session below it.
fn validate<'a>(
    state: &State,
    req: &'a NewIamRoleChained,
    edited: Option<SessionId>,
) -> Result<Valid<'a>> {
    let name = required(Field::Name, &req.name)?;
    check_parent(state, req.parent_session_id, edited)?;
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
    Ok(Valid {
        name,
        profile,
        kind: SessionKind::IamRoleChained {
            role_arn: role_arn.to_string(),
            parent_session_id: req.parent_session_id,
            role_session_name,
            external_id,
        },
    })
}

/// The parent must exist; for an edit of `edited`, it must also not make a loop. A parent that
/// an `Editing` reservation holds gives `Error::ProfilePending`. A delete reserves its session
/// and every session below it, so a check of the parent alone also finds a delete of any
/// session above the parent.
fn check_parent(state: &State, parent: SessionId, edited: Option<SessionId>) -> Result<()> {
    let ws = &state.ws;
    let message = if ws.session(parent).is_none() {
        "session not found"
    } else if edited == Some(parent) {
        "must not be the session itself"
    } else if edited.is_some_and(|id| ws.descendants(id).contains(&parent)) {
        "would make a loop of chained sessions"
    } else if state.editing.contains(&parent) {
        return Err(parent_editing_error());
    } else {
        return Ok(());
    };
    Err(Error::InvalidInput {
        field: Field::ParentSession,
        message: message.into(),
    })
}

/// An edit has the fields of a create request; one validation serves both.
fn as_new(req: IamRoleChainedUpdate) -> NewIamRoleChained {
    let IamRoleChainedUpdate {
        name,
        parent_session_id,
        role_arn,
        role_session_name,
        external_id,
        profile_name,
        region,
    } = req;
    NewIamRoleChained {
        name,
        parent_session_id,
        role_arn,
        role_session_name,
        external_id,
        profile_name,
        region,
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
