//! Text edits of `~/.aws/credentials` (spec 9.3).
//!
//! The file is edited as text, section by section. Bytes outside the edited section stay the same.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The four keys of a section that Leapp RS writes.
pub(crate) struct SectionBody {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub region: String,
}

impl std::fmt::Debug for SectionBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionBody")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

pub(crate) struct CredentialsFile {
    path: PathBuf,
}

impl CredentialsFile {
    pub fn new(home: &Path) -> Self {
        CredentialsFile {
            path: home.join(".aws").join("credentials"),
        }
    }

    /// Count of `[profile]` headers. Task 4 uses it to refuse a duplicate section.
    #[allow(dead_code)]
    pub fn sections_named(&self, profile: &str) -> Result<usize> {
        let text = self.read()?;
        Ok(lines(&text)
            .filter(|l| header_name(l) == Some(profile.trim()))
            .count())
    }

    /// Text of the first section with this name, header included, as it is in the file.
    /// Blank lines at the end of the section are not part of it.
    pub fn section_text(&self, profile: &str) -> Result<Option<String>> {
        let text = self.read()?;
        Ok(find_section(&text, profile).map(|(start, end)| text[start..end].to_string()))
    }

    /// Replaces the first section with this name in place, or appends a new one.
    pub fn write_section(&self, profile: &str, body: &SectionBody) -> Result<()> {
        check_profile(profile)?;
        for value in [
            &body.access_key_id,
            &body.secret_access_key,
            &body.session_token,
            &body.region,
        ] {
            if value.contains(['\r', '\n']) {
                return Err(Error::InvalidInput {
                    field: "credentials",
                    message: "a value contains a line break".into(),
                });
            }
        }
        let text = self.read()?;
        let eol = eol_of(&text);
        let section = format!(
            "[{profile}]{eol}aws_access_key_id = {}{eol}aws_secret_access_key = {}{eol}aws_session_token = {}{eol}region = {}{eol}",
            body.access_key_id, body.secret_access_key, body.session_token, body.region
        );
        self.write(&replace_or_append(&text, profile, &section))
    }

    /// Replaces the first section with this name with saved text, or appends it.
    pub fn write_raw_section(&self, profile: &str, text: &str) -> Result<()> {
        check_profile(profile)?;
        let current = self.read()?;
        let eol = eol_of(&current);
        let mut section = text.to_string();
        if !section.ends_with('\n') {
            section.push_str(eol);
        }
        self.write(&replace_or_append(&current, profile, &section))
    }

    /// Removes the first section with this name. Other bytes stay the same.
    pub fn remove_section(&self, profile: &str) -> Result<()> {
        let text = self.read()?;
        let Some((start, end)) = find_section(&text, profile) else {
            return Ok(());
        };
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..start]);
        out.push_str(&text[end..]);
        self.write(&out)
    }

    fn read(&self) -> Result<String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomic write: temp file in the same folder, mode 0600, then rename.
    fn write(&self, text: &str) -> Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        create_private_dir(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        set_private_mode(file.path())?;
        file.write_all(text.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }
}

fn check_profile(profile: &str) -> Result<()> {
    if profile.trim().is_empty() || profile.contains(['[', ']', '\r', '\n']) {
        return Err(Error::InvalidInput {
            field: "profile",
            message: "must not be empty or contain [, ], or line breaks".into(),
        });
    }
    Ok(())
}

/// Lines with their line break.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
}

/// The name in a header line such as `[name]` or `[name] ; comment`. `None` for other lines.
fn header_name(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix('[')?;
    let (name, _) = rest.split_once(']')?;
    Some(name.trim())
}

fn is_header(line: &str) -> bool {
    line.trim().starts_with('[')
}

/// Byte range of the first section with this name, without blank lines at its end.
fn find_section(text: &str, profile: &str) -> Option<(usize, usize)> {
    let wanted = profile.trim();
    let mut offset = 0;
    let mut start = None;
    let mut end = 0;
    for line in lines(text) {
        let line_end = offset + line.len();
        match start {
            None => {
                if header_name(line) == Some(wanted) {
                    start = Some(offset);
                    end = line_end;
                }
            }
            Some(_) => {
                if is_header(line) {
                    break;
                }
                if !line.trim().is_empty() {
                    end = line_end;
                }
            }
        }
        offset = line_end;
    }
    start.map(|s| (s, end))
}

fn replace_or_append(text: &str, profile: &str, section: &str) -> String {
    let mut out = String::with_capacity(text.len() + section.len());
    match find_section(text, profile) {
        Some((start, end)) => {
            out.push_str(&text[..start]);
            out.push_str(section);
            out.push_str(&text[end..]);
        }
        None => {
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(eol_of(text));
            }
            out.push_str(section);
        }
    }
    out
}

/// `\r\n` if the first line break of the file is `\r\n`, else `\n`.
fn eol_of(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) -> Result<()> {
    Ok(())
}
