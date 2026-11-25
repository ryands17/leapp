use std::io::Write;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{SessionStatus, WORKSPACE_VERSION, Workspace};

/// Reads the workspace file.
///
/// A missing file gives a new workspace. A file with a newer version is refused and not changed.
/// A file that is not valid JSON, or does not match the model, is moved to
/// `workspace.json.broken-<timestamp>` and a new workspace is used.
pub fn load(path: &Path) -> Result<Workspace> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Workspace::new()),
        Err(e) => return Err(e.into()),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return move_aside(path),
    };
    if let Some(version) = value.get("version").and_then(|v| v.as_u64())
        && version > u64::from(WORKSPACE_VERSION)
    {
        return Err(Error::UnsupportedVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }
    match serde_json::from_value(value) {
        Ok(ws) => Ok(ws),
        Err(_) => move_aside(path),
    }
}

fn move_aside(path: &Path) -> Result<Workspace> {
    let stamp = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%S%.3fZ");
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".broken-{stamp}"));
    std::fs::rename(path, path.with_file_name(name))?;
    Ok(Workspace::new())
}

/// Writes the workspace file. The write is atomic: temp file in the same folder, then rename.
///
/// `Pending` is a state of this run only, so every `Pending` session is written as `Inactive`.
/// This is the only place that applies the rule, so no writer can put `pending` on disk.
pub fn save(path: &Path, workspace: &Workspace) -> Result<()> {
    let mut workspace = workspace.clone();
    for s in &mut workspace.sessions {
        if s.status == SessionStatus::Pending {
            s.status = SessionStatus::Inactive;
        }
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(&serde_json::to_vec_pretty(&workspace)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
}
