use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use super::error::{ToolError, io};

#[derive(Clone, Default)]
pub struct MutationCoordinator {
    locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl MutationCoordinator {
    pub fn with_path_lock<T>(
        &self,
        path: &Path,
        operation: impl FnOnce() -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let lock = {
            let mut locks = self.locks.lock().unwrap();
            locks
                .entry(path.to_path_buf())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().unwrap();
        operation()
    }

    pub fn replace_if_unchanged(
        &self,
        path: &Path,
        expected: &[u8],
        replacement: &[u8],
    ) -> Result<(), ToolError> {
        let current = std::fs::read(path).map_err(|e| io("read before replace", path, e))?;
        if current != expected {
            return Err(ToolError::StaleReference {
                path: path.to_path_buf(),
                message: "file changed while the mutation was prepared".into(),
            });
        }
        self.atomic_replace(path, replacement)
    }

    pub fn atomic_replace(&self, path: &Path, replacement: &[u8]) -> Result<(), ToolError> {
        let parent = path
            .parent()
            .ok_or_else(|| ToolError::InvalidEdit(format!("{} has no parent", path.display())))?;
        std::fs::create_dir_all(parent).map_err(|e| io("create parent", parent, e))?;
        let name = path
            .file_name()
            .ok_or_else(|| ToolError::InvalidEdit(format!("{} has no file name", path.display())))?
            .to_string_lossy();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let temporary = parent.join(format!(".{name}.tau-{stamp}.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| io("create temporary", &temporary, e))?;
            if let Ok(metadata) = std::fs::metadata(path) {
                let _ = std::fs::set_permissions(&temporary, metadata.permissions());
            }
            std::io::Write::write_all(&mut file, replacement)
                .map_err(|e| io("write temporary", &temporary, e))?;
            file.sync_all()
                .map_err(|e| io("sync temporary", &temporary, e))?;
            std::fs::rename(&temporary, path).map_err(|e| io("rename temporary", path, e))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}
