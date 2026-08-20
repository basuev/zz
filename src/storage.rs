use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

#[derive(Debug, Serialize, Deserialize)]
struct DraftRecord {
    workspace: PathBuf,
    seed_hash: String,
    text: String,
    cursor: usize,
    updated_at_ms: u128,
}

#[derive(Debug)]
pub struct RecoveredDraft {
    pub text: String,
    pub cursor: usize,
}

#[derive(Debug)]
pub struct DraftStore {
    path: PathBuf,
    workspace: PathBuf,
    seed_hash: String,
}

impl DraftStore {
    pub fn new(workspace: &Path, seed: &str) -> Result<Self> {
        let project = ProjectDirs::from("dev", "zz", "zz")
            .context("could not determine the application support directory")?;
        let drafts = project.data_dir().join("drafts");
        fs::create_dir_all(&drafts)
            .with_context(|| format!("could not create {}", drafts.display()))?;
        set_private_directory_permissions(&drafts)?;

        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let seed_hash = digest(seed.as_bytes());
        let key = digest(format!("{}\0{}", workspace.display(), seed_hash).as_bytes());

        Ok(Self {
            path: drafts.join(format!("{key}.json")),
            workspace,
            seed_hash,
        })
    }

    pub fn recover(&self) -> Result<Option<RecoveredDraft>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read {}", self.path.display()));
            }
        };
        let record: DraftRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid draft {}", self.path.display()))?;
        if record.workspace != self.workspace || record.seed_hash != self.seed_hash {
            return Ok(None);
        }
        Ok(Some(RecoveredDraft {
            text: record.text,
            cursor: record.cursor,
        }))
    }

    pub fn save(&self, text: &str, cursor: usize) -> Result<()> {
        let record = DraftRecord {
            workspace: self.workspace.clone(),
            seed_hash: self.seed_hash.clone(),
            text: text.to_owned(),
            cursor,
            updated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        };
        let bytes = serde_json::to_vec(&record)?;
        atomic_write(&self.path, &bytes, None)
    }

    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => sync_parent(&self.path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("could not remove {}", self.path.display()))
            }
        }
    }
}

pub fn replace_input_file(path: &Path, text: &str) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("could not inspect input file {}", path.display()))?;
    atomic_write(path, text.as_bytes(), Some(metadata.permissions()))
        .with_context(|| format!("could not commit prompt to {}", path.display()))
}

fn atomic_write(path: &Path, bytes: &[u8], permissions: Option<fs::Permissions>) -> Result<()> {
    let parent = path
        .parent()
        .context("target path has no parent directory")?;
    fs::create_dir_all(parent)?;

    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    } else {
        set_private_file_permissions(temporary.as_file())?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not atomically replace {}", path.display()))?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn atomic_replace_changes_content() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("prompt.txt");
        fs::write(&path, "before").unwrap();
        replace_input_file(&path, "after").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "after");
    }
}
