//! Start and stop of sessions (spec 10.1, 10.2).

mod iam_user;

use uuid::Uuid;

use crate::Leapp;
use crate::aws_files::CredentialsFile;
use crate::error::{Error, Result};
use crate::model::{ManagedProfile, SessionId, SessionKind, SessionStatus, Workspace};
use crate::store;

impl Leapp {
    /// Starts a session: stops other sessions on the same profile, gets credentials, writes the
    /// profile section. On error the status is `Inactive` and the credentials file is unchanged.
    pub async fn start(&self, id: SessionId) -> Result<()> {
        let (profile_id, profile, to_stop) = {
            let ws = self.workspace.lock().unwrap();
            let session = ws.session(id).ok_or(Error::NotFound(id))?;
            let profile = profile_name(&ws, session.profile_id)?;
            if pending_on(&ws, session.profile_id) {
                return Err(pending_error());
            }
            let to_stop: Vec<SessionId> = ws
                .sessions
                .iter()
                .filter(|s| s.profile_id == session.profile_id && s.status == SessionStatus::Active)
                .map(|s| s.id)
                .collect();
            (session.profile_id, profile, to_stop)
        };
        for other in to_stop {
            self.stop(other).await?;
        }
        {
            let mut ws = self.workspace.lock().unwrap();
            if pending_on(&ws, profile_id) {
                return Err(pending_error());
            }
            set_status(&mut ws, id, SessionStatus::Pending);
        }

        let result = self.activate(id, &profile).await;
        if result.is_err() {
            let mut ws = self.workspace.lock().unwrap();
            set_status(&mut ws, id, SessionStatus::Inactive);
        }
        result
    }

    /// Stops a session: removes its profile section. Does nothing if it is `Inactive`.
    pub async fn stop(&self, id: SessionId) -> Result<()> {
        let mut ws = self.workspace.lock().unwrap();
        let session = ws.session(id).ok_or(Error::NotFound(id))?;
        if session.status == SessionStatus::Inactive {
            return Ok(());
        }
        let profile = profile_name(&ws, session.profile_id)?;
        let file = CredentialsFile::new(&self.config.home_dir);

        let mut next = ws.clone();
        let mut previous_section = None;
        if next.managed_profiles.get(&profile).map(|m| m.session_id) == Some(id) {
            previous_section = file.section_text(&profile)?;
            file.remove_section(&profile)?;
            next.managed_profiles.remove(&profile);
        }
        if let Some(s) = next.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Inactive;
            s.start_time = None;
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            restore(&file, &profile, previous_section);
            return Err(e);
        }
        *ws = next;
        Ok(())
    }

    /// Steps 4 to 6 of spec 10.1. Changes the file and the workspace together or not at all.
    async fn activate(&self, id: SessionId, profile: &str) -> Result<()> {
        let session = {
            let ws = self.workspace.lock().unwrap();
            ws.session(id).ok_or(Error::NotFound(id))?.clone()
        };
        let (body, expiration) = match session.kind {
            SessionKind::IamUser { .. } => self.iam_user_credentials(&session).await?,
        };

        let mut ws = self.workspace.lock().unwrap();
        let file = CredentialsFile::new(&self.config.home_dir);
        let previous_section = file.section_text(profile)?;
        file.write_section(profile, &body)?;

        let mut next = ws.clone();
        next.managed_profiles.insert(
            profile.to_string(),
            ManagedProfile {
                session_id: id,
                had_original: false,
            },
        );
        if let Some(s) = next.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Active;
            s.start_time = Some(self.config.clock.now());
            s.session_token_expiration = Some(expiration);
        }
        if let Err(e) = store::save(&self.workspace_path, &next) {
            restore(&file, profile, previous_section);
            return Err(e);
        }
        *ws = next;
        Ok(())
    }
}

fn profile_name(ws: &Workspace, profile_id: Uuid) -> Result<String> {
    ws.profile_name(profile_id)
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidInput {
            field: "profile",
            message: "the profile of the session does not exist".into(),
        })
}

fn pending_on(ws: &Workspace, profile_id: Uuid) -> bool {
    ws.sessions
        .iter()
        .any(|s| s.profile_id == profile_id && s.status == SessionStatus::Pending)
}

fn pending_error() -> Error {
    Error::ProfilePending("Another session with the same named profile is starting".into())
}

fn set_status(ws: &mut Workspace, id: SessionId, status: SessionStatus) {
    if let Some(s) = ws.sessions.iter_mut().find(|s| s.id == id) {
        s.status = status;
    }
}

/// Puts the section back as it was, after a failed save. The error of the save is more important
/// than an error here, so this one is dropped.
fn restore(file: &CredentialsFile, profile: &str, previous_section: Option<String>) {
    let _ = match previous_section {
        Some(text) => file.write_raw_section(profile, &text),
        None => file.remove_section(profile),
    };
}
