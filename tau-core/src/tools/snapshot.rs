use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Serialize)]
struct SnapshotManifest {
    id: String,
    entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct SnapshotEntry {
    path: String,
    kind: &'static str,
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
            self.collect(path, &mut entries)?;
        }
        let manifest_seed =
            serde_json::to_vec(&entries).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        let id = digest(&manifest_seed);
        let snapshot_dir = self.root.join(&id);
        let blobs = snapshot_dir.join("blobs");
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
        let manifest_path = snapshot_dir.join("manifest.json");
        let encoded =
            serde_json::to_vec_pretty(&manifest).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        std::fs::write(&manifest_path, encoded).map_err(|e| ToolError::Snapshot(e.to_string()))?;
        Ok(SnapshotCapture {
            id,
            manifest: manifest_path,
            entries: entries.len(),
        })
    }

    fn collect(&self, path: &Path, entries: &mut Vec<SnapshotEntry>) -> Result<(), ToolError> {
        let relative = path
            .strip_prefix(&self.base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(SnapshotEntry {
                    path: relative,
                    kind: "missing",
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
                kind: "directory",
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
                kind: "other",
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
                kind: "file",
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
            kind: "file",
            existed: true,
            bytes: bytes.len() as u64,
            digest: Some(digest(&bytes)),
            skipped: false,
        });
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}
