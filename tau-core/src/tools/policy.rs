use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Result;

use super::error::{ToolError, io};

#[derive(Clone)]
pub struct AccessPolicy {
    roots: Arc<RwLock<BTreeSet<PathBuf>>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub path: PathBuf,
    pub root: PathBuf,
}

impl AccessPolicy {
    pub fn for_cwd(cwd: impl AsRef<Path>) -> Result<Self> {
        let root =
            canonicalize_target(cwd.as_ref()).map_err(|e| io("canonicalize", cwd.as_ref(), e))?;
        Ok(Self {
            roots: Arc::new(RwLock::new(BTreeSet::from([root]))),
        })
    }

    pub fn register_root(&self, root: impl AsRef<Path>) -> Result<()> {
        let root =
            canonicalize_target(root.as_ref()).map_err(|e| io("canonicalize", root.as_ref(), e))?;
        self.roots.write().unwrap().insert(root);
        Ok(())
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.read().unwrap().iter().cloned().collect()
    }

    pub fn resolve(
        &self,
        cwd: &Path,
        path: impl AsRef<Path>,
        operation: &'static str,
    ) -> Result<ResolvedPath, ToolError> {
        let requested = path.as_ref();
        let absolute = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        };
        let resolved =
            canonicalize_target(&absolute).map_err(|e| io("canonicalize", &absolute, e))?;
        let roots = self.roots();
        let Some(root) = roots.iter().find(|root| is_within(root, &resolved)) else {
            return Err(ToolError::ApprovalNeeded {
                operation,
                path: resolved,
                roots,
            });
        };
        Ok(ResolvedPath {
            path: resolved,
            root: root.clone(),
        })
    }
}

fn canonicalize_target(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path);
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        suffix.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }
    let mut result = std::fs::canonicalize(ancestor)?;
    for part in suffix.iter().rev() {
        result.push(part);
    }
    Ok(result)
}

fn is_within(root: &Path, path: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_does_not_accept_sibling_prefixes() {
        let root = Path::new("/tmp/work");
        assert!(is_within(root, Path::new("/tmp/work/file")));
        assert!(!is_within(root, Path::new("/tmp/worktree/file")));
    }
}
