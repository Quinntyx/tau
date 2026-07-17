use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::error::ToolError;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotCapture {
    pub id: String,
    pub manifest: PathBuf,
    pub entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotManifest {
    id: String,
    entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotEntry {
    path: String,
    kind: String,
    existed: bool,
    bytes: u64,
    digest: Option<String>,
    skipped: bool,
}

#[derive(Debug, Clone)]
pub struct SnapshotStore {
    base: PathBuf,
    root: PathBuf,
}

impl SnapshotStore {
    pub fn for_cwd(cwd: impl Into<PathBuf>) -> Self {
        let base = cwd.into();
        Self {
            root: base.join(".tau").join("snapshots"),
            base,
        }
    }

    pub fn capture_paths(&self, paths: &[PathBuf]) -> Result<SnapshotCapture, ToolError> {
        let mut entries = Vec::new();
        for path in paths {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                self.base.join(path)
            };
            if path
                .components()
                .any(|component| component == Component::ParentDir)
                || !path.starts_with(&self.base)
            {
                return Err(ToolError::Snapshot("path is outside snapshot root".into()));
            }
            self.collect(&path, &mut entries)?;
        }
        let manifest_seed =
            serde_json::to_vec(&entries).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        let id = digest(&manifest_seed);
        std::fs::create_dir_all(&self.root).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        let snapshot_dir = self.root.join(&id);
        let temp_dir = self.root.join(format!(".{id}.tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let blobs = temp_dir.join("blobs");
        std::fs::create_dir_all(&blobs).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        let manifest = SnapshotManifest {
            id: id.clone(),
            entries: entries.clone(),
        };
        for entry in &entries {
            let Some(digest) = &entry.digest else {
                continue;
            };
            let path = self.base.join(&entry.path);
            let bytes = std::fs::read(&path).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            let blob = blobs.join(digest);
            if !blob.exists() {
                std::fs::write(blob, bytes).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            }
        }
        let manifest_path = temp_dir.join("manifest.json");
        let encoded =
            serde_json::to_vec_pretty(&manifest).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        std::fs::write(&manifest_path, encoded).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        if snapshot_dir.exists() {
            let _ = std::fs::remove_dir_all(&temp_dir);
        } else {
            std::fs::rename(&temp_dir, &snapshot_dir)
                .map_err(|e| ToolError::Snapshot(e.to_string()))?;
        }
        let manifest_path = snapshot_dir.join("manifest.json");
        Ok(SnapshotCapture {
            id,
            manifest: manifest_path,
            entries: entries.len(),
        })
    }

    pub fn list(&self) -> Result<Vec<SnapshotCapture>, ToolError> {
        let mut captures = Vec::new();
        if !self.root.exists() {
            return Ok(captures);
        }
        for entry in
            std::fs::read_dir(&self.root).map_err(|e| ToolError::Snapshot(e.to_string()))?
        {
            let path = entry
                .map_err(|e| ToolError::Snapshot(e.to_string()))?
                .path();
            let manifest_path = path.join("manifest.json");
            if !manifest_path.is_file() {
                continue;
            }
            let bytes =
                std::fs::read(&manifest_path).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            let manifest: SnapshotManifest =
                serde_json::from_slice(&bytes).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            captures.push(SnapshotCapture {
                id: manifest.id,
                manifest: manifest_path,
                entries: manifest.entries.len(),
            });
        }
        captures.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(captures)
    }

    pub fn restore(&self, id: &str) -> Result<usize, ToolError> {
        if id.len() != 40 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolError::Snapshot("invalid snapshot id".into()));
        }
        let snapshot_dir = self.root.join(id);
        let manifest_path = snapshot_dir.join("manifest.json");
        let bytes =
            std::fs::read(&manifest_path).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        let manifest: SnapshotManifest =
            serde_json::from_slice(&bytes).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        if manifest.id != id {
            return Err(ToolError::Snapshot("snapshot id mismatch".into()));
        }
        for entry in &manifest.entries {
            validate_relative(&entry.path)?;
            if let Some(digest) = &entry.digest {
                if digest.len() != 40 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(ToolError::Snapshot("invalid blob digest".into()));
                }
                if !snapshot_dir.join("blobs").join(digest).is_file() {
                    return Err(ToolError::Snapshot("missing snapshot blob".into()));
                }
            }
        }
        let mut restored = 0;
        for entry in manifest.entries {
            let target = self.base.join(&entry.path);
            if entry.kind == "directory" {
                std::fs::create_dir_all(&target).map_err(|e| ToolError::Snapshot(e.to_string()))?;
                continue;
            }
            if !entry.existed {
                if target.is_file() {
                    std::fs::remove_file(&target)
                        .map_err(|e| ToolError::Snapshot(e.to_string()))?;
                }
                continue;
            }
            let Some(digest) = entry.digest else {
                continue;
            };
            let source = snapshot_dir.join("blobs").join(digest);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            }
            std::fs::copy(source, target).map_err(|e| ToolError::Snapshot(e.to_string()))?;
            restored += 1;
        }
        Ok(restored)
    }

    fn collect(&self, path: &Path, entries: &mut Vec<SnapshotEntry>) -> Result<(), ToolError> {
        let relative = path
            .strip_prefix(&self.base)
            .map_err(|_| ToolError::Snapshot("path is outside snapshot root".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(SnapshotEntry {
                    path: relative,
                    kind: "missing".into(),
                    existed: false,
                    bytes: 0,
                    digest: None,
                    skipped: false,
                });
                return Ok(());
            }
            Err(error) => return Err(ToolError::Snapshot(error.to_string())),
        };
        if metadata.is_dir() {
            entries.push(SnapshotEntry {
                path: relative,
                kind: "directory".into(),
                existed: true,
                bytes: 0,
                digest: None,
                skipped: false,
            });
            for child in std::fs::read_dir(path).map_err(|e| ToolError::Snapshot(e.to_string()))? {
                let child = child.map_err(|e| ToolError::Snapshot(e.to_string()))?;
                if matches!(child.file_name().to_str(), Some(".git" | ".tau")) {
                    continue;
                }
                self.collect(&child.path(), entries)?;
            }
            return Ok(());
        }
        if !metadata.is_file() {
            entries.push(SnapshotEntry {
                path: relative,
                kind: "other".into(),
                existed: true,
                bytes: metadata.len(),
                digest: None,
                skipped: true,
            });
            return Ok(());
        }
        if metadata.len() > MAX_FILE_BYTES {
            entries.push(SnapshotEntry {
                path: relative,
                kind: "file".into(),
                existed: true,
                bytes: metadata.len(),
                digest: None,
                skipped: true,
            });
            return Ok(());
        }
        let bytes = std::fs::read(path).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        entries.push(SnapshotEntry {
            path: relative,
            kind: "file".into(),
            existed: true,
            bytes: bytes.len() as u64,
            digest: Some(digest(&bytes)),
            skipped: false,
        });
        Ok(())
    }
}

fn validate_relative(path: &str) -> Result<(), ToolError> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || path == "."
    {
        return Err(ToolError::Snapshot("invalid snapshot path".into()));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_replays_captured_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file.txt");
        std::fs::write(&path, "before").unwrap();
        let store = SnapshotStore::for_cwd(root.path());
        let capture = store.capture_paths(std::slice::from_ref(&path)).unwrap();
        std::fs::write(&path, "after").unwrap();
        assert_eq!(store.restore(&capture.id).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn restore_rejects_traversal_manifest() {
        let root = tempfile::tempdir().unwrap();
        let store = SnapshotStore::for_cwd(root.path());
        let id = "0123456789abcdef0123456789abcdef01234567";
        let dir = root.path().join(".tau/snapshots").join(id);
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::json!({"id": id, "entries": [{"path":"../escape","kind":"file","existed":true,"bytes":0,"digest":null,"skipped":true}]}).to_string(),
        ).unwrap();
        assert!(store.restore(id).is_err());
    }

    #[test]
    fn capture_rejects_paths_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("outside.txt");
        std::fs::write(&path, "secret").unwrap();
        let store = SnapshotStore::for_cwd(root.path());
        assert!(store.capture_paths(&[path]).is_err());
        assert!(
            store
                .capture_paths(&[PathBuf::from("../outside.txt")])
                .is_err()
        );
    }
}
