//! Reviewable snapshot transactions: the filesystem is changed only by an
//! explicit decision, and every decision can be undone or redone.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: Vec<String>,
    pub new_start: usize,
    pub new_lines: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffFile {
    pub path: PathBuf,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
    pub renamed_from: Option<PathBuf>,
    pub deleted: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileDecision {
    Accept,
    Reject,
    Delete,
    Restore,
    Force,
}
#[derive(Debug, Error)]
pub enum TransactionError {
    #[error("transaction conflict at {0}")]
    Conflict(PathBuf),
    #[error("path error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transaction has no undo state")]
    NoUndo,
    #[error("transaction has no redo state")]
    NoRedo,
}
#[derive(Debug, Clone)]
struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}
pub struct SnapshotTransaction {
    root: PathBuf,
    before: HashMap<PathBuf, Option<Vec<u8>>>,
    changes: Vec<Change>,
    cursor: usize,
}
impl SnapshotTransaction {
    pub fn begin(root: impl Into<PathBuf>, paths: &[PathBuf]) -> Result<Self, TransactionError> {
        let root = std::fs::canonicalize(root.into())?;
        let mut before = HashMap::new();
        for p in paths {
            ensure_within(&root, p)?;
            before.insert(p.clone(), read(p)?);
        }
        Ok(Self {
            root,
            before,
            changes: Vec::new(),
            cursor: 0,
        })
    }
    pub fn diff(&mut self) -> Result<Vec<DiffFile>, TransactionError> {
        self.changes.clear();
        for (path, old) in &self.before {
            let new = read(path)?;
            if old != &new {
                self.changes.push(Change {
                    path: path.clone(),
                    before: old.clone(),
                    after: new,
                });
            }
        }
        self.cursor = self.changes.len();
        self.changes.iter().map(|c| self.file_diff(c)).collect()
    }
    fn file_diff(&self, c: &Change) -> Result<DiffFile, TransactionError> {
        let binary = c.before.as_ref().map(|b| b.contains(&0)).unwrap_or(false)
            || c.after.as_ref().map(|b| b.contains(&0)).unwrap_or(false);
        let old = c
            .before
            .as_ref()
            .map(|b| {
                String::from_utf8_lossy(b)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let new = c
            .after
            .as_ref()
            .map(|b| {
                String::from_utf8_lossy(b)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(DiffFile {
            path: c.path.clone(),
            hunks: if binary {
                vec![]
            } else {
                vec![DiffHunk {
                    old_start: 1,
                    old_lines: old,
                    new_start: 1,
                    new_lines: new,
                }]
            },
            binary,
            renamed_from: None,
            deleted: c.after.is_none(),
        })
    }
    pub fn apply(
        &mut self,
        decisions: &[(PathBuf, FileDecision)],
        force: bool,
    ) -> Result<usize, TransactionError> {
        let mut n = 0;
        // Preflight every decision before mutating anything.  A failed later
        // decision must not leave a partially applied transaction behind.
        for (path, decision) in decisions {
            let c = self
                .changes
                .iter()
                .find(|c| &c.path == path)
                .ok_or_else(|| TransactionError::Conflict(path.clone()))?;
            if !force && *decision != FileDecision::Force && read(path)? != c.after {
                return Err(TransactionError::Conflict(path.clone()));
            }
            ensure_within(&self.root, path)?;
        }
        for (path, decision) in decisions {
            let c = self
                .changes
                .iter()
                .find(|c| &c.path == path)
                .expect("preflight checked");
            let target = match decision {
                // Accept materializes the proposed state. Restore is the
                // explicit operation for putting the snapshot back.
                FileDecision::Accept => c.after.clone(),
                FileDecision::Force => c.before.clone(),
                FileDecision::Restore => c.before.clone(),
                FileDecision::Delete => None,
                FileDecision::Reject => c.before.clone(),
            };
            write(path, target)?;
            n += 1;
        }
        Ok(n)
    }
    pub fn undo(&mut self) -> Result<(), TransactionError> {
        if self.cursor == 0 {
            return Err(TransactionError::NoUndo);
        }
        self.cursor -= 1;
        let c = &self.changes[self.cursor];
        write(&c.path, c.before.clone())?;
        Ok(())
    }
    pub fn redo(&mut self) -> Result<(), TransactionError> {
        if self.cursor >= self.changes.len() {
            return Err(TransactionError::NoRedo);
        }
        let c = &self.changes[self.cursor];
        write(&c.path, c.after.clone())?;
        self.cursor += 1;
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    match std::fs::read(path) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn write(path: &Path, bytes: Option<Vec<u8>>) -> Result<(), std::io::Error> {
    match bytes {
        Some(v) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temp = path.with_file_name(format!(
                ".{}.tau-tmp-{}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                std::process::id()
            ));
            std::fs::write(&temp, v)?;
            std::fs::rename(temp, path)
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

fn ensure_within(root: &Path, path: &Path) -> Result<(), TransactionError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate.parent().unwrap_or(root);
    let canonical_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    if !canonical_parent.starts_with(root) {
        return Err(TransactionError::Conflict(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diff_restore_conflict_and_undo_redo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        let mut tx = SnapshotTransaction::begin(dir.path(), std::slice::from_ref(&file)).unwrap();
        std::fs::write(&file, "one\nchanged\n").unwrap();
        assert_eq!(tx.diff().unwrap()[0].hunks[0].old_lines[1], "two");
        std::fs::write(&file, "external\n").unwrap();
        assert!(matches!(
            tx.apply(&[(file.clone(), FileDecision::Accept)], false),
            Err(TransactionError::Conflict(_))
        ));
        tx.apply(&[(file.clone(), FileDecision::Force)], true)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\ntwo\n");
        std::fs::write(&file, "new\n").unwrap();
        tx.diff().unwrap();
        tx.undo().unwrap();
        tx.redo().unwrap();
    }

    #[test]
    fn accept_applies_proposal_and_reject_restores_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "before").unwrap();
        let mut tx = SnapshotTransaction::begin(dir.path(), std::slice::from_ref(&file)).unwrap();
        std::fs::write(&file, "proposal").unwrap();
        tx.diff().unwrap();
        tx.apply(&[(file.clone(), FileDecision::Accept)], false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "proposal");
        std::fs::write(&file, "proposal").unwrap();
        tx.apply(&[(file.clone(), FileDecision::Reject)], false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "before");
    }
}
