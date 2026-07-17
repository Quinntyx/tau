//! Explicit, non-destructive Git orchestration for tau sessions.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GitTopology {
    Direct,
    Grouped,
    NonGit,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryManifest {
    pub root: PathBuf,
    pub topology: GitTopology,
    pub children: Vec<PathBuf>,
    pub tau_metadata: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWorktree {
    pub branch: String,
    pub path: PathBuf,
    pub repository: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationPreview {
    pub repository: PathBuf,
    pub source_branch: String,
    pub target_branch: String,
    pub commits: Vec<String>,
    pub conflicts: bool,
}
pub struct GitWorkspace {
    pub manifest: RepositoryManifest,
}
impl GitWorkspace {
    pub fn initialize(
        root: impl Into<PathBuf>,
        topology: GitTopology,
        children: Vec<PathBuf>,
    ) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        match topology {
            GitTopology::Direct => {
                if !is_repo(&root) {
                    git(&root, &["init"])?;
                }
            }
            GitTopology::Grouped => {
                for child in &children {
                    std::fs::create_dir_all(child)?;
                    if !is_repo(child) {
                        git(child, &["init"])?;
                    }
                }
            }
            GitTopology::NonGit => {}
        }
        let tau = root.join(".tau");
        std::fs::create_dir_all(&tau)?;
        let metadata = tau.join("metadata.git");
        if !is_repo(&metadata) {
            std::fs::create_dir_all(&metadata)?;
            git(&metadata, &["init", "--bare"])?;
        }
        Ok(Self {
            manifest: RepositoryManifest {
                root,
                topology,
                children,
                tau_metadata: metadata,
            },
        })
    }
    pub fn managed_worktree(&self, model: &str, repository: &Path) -> Result<ManagedWorktree> {
        let branch = managed_name(model);
        let folder = self.manifest.root.join(&branch);
        if !branch_exists(repository, &branch) {
            git(repository, &["branch", &branch])?;
        }
        if !folder.exists() {
            git(
                repository,
                &[
                    "worktree",
                    "add",
                    folder.to_str().context("non-UTF8 worktree path")?,
                    &branch,
                ],
            )?;
        }
        Ok(ManagedWorktree {
            branch,
            path: folder,
            repository: repository.to_path_buf(),
        })
    }
    pub fn stage_tau_touched(&self, repository: &Path, paths: &[PathBuf]) -> Result<()> {
        for p in paths {
            git(
                repository,
                &["add", "--", p.to_str().context("non-UTF8 path")?],
            )?;
        }
        Ok(())
    }
    pub fn commit(&self, repository: &Path, message: &str, paths: &[PathBuf]) -> Result<String> {
        self.stage_tau_touched(repository, paths)?;
        let status = git(repository, &["diff", "--cached", "--quiet"]);
        if status.is_ok() {
            bail!("nothing staged for commit");
        }
        git(repository, &["commit", "-m", message])?;
        Ok(
            String::from_utf8_lossy(&git_output(repository, &["rev-parse", "HEAD"])?.stdout)
                .trim()
                .to_owned(),
        )
    }
    pub fn preview_integration(
        &self,
        repository: &Path,
        source: &str,
        target: &str,
    ) -> Result<IntegrationPreview> {
        let commits = String::from_utf8_lossy(
            &git_output(
                repository,
                &["log", "--format=%H", &format!("{target}..{source}")],
            )?
            .stdout,
        )
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let conflicts = git(repository, &["merge-tree", target, source])
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("<<<<<<<"))
            .unwrap_or(false);
        Ok(IntegrationPreview {
            repository: repository.to_path_buf(),
            source_branch: source.into(),
            target_branch: target.into(),
            commits,
            conflicts,
        })
    }
    /// Integrate only when explicitly requested; this never checks out or rewrites
    /// the user's branch before the caller asks for this method.
    pub fn integrate(
        &self,
        repository: &Path,
        source: &str,
        target: &str,
        force: bool,
    ) -> Result<()> {
        let preview = self.preview_integration(repository, source, target)?;
        if preview.conflicts && !force {
            bail!("integration has conflicts; resolve or force explicitly")
        }
        git(repository, &["checkout", target])?;
        git(repository, &["merge", "--no-ff", source])?;
        Ok(())
    }
}
pub fn managed_name(model: &str) -> String {
    let mut s = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_owned();
    if s.is_empty() {
        s = "model".into();
    }
    if s.len() > 64 {
        s.truncate(64);
    }
    format!("tau/{s}")
}
fn is_repo(path: &Path) -> bool {
    path.join(".git").exists() || path.join("HEAD").exists()
}
fn branch_exists(path: &Path, branch: &str) -> bool {
    git(
        path,
        &["show-ref", "--verify", &format!("refs/heads/{branch}")],
    )
    .is_ok()
}
fn git(path: &Path, args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .with_context(|| format!("running git in {}", path.display()))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out)
}
fn git_output(path: &Path, args: &[&str]) -> Result<std::process::Output> {
    git(path, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_branch_names_are_safe_and_stable() {
        assert_eq!(managed_name("Claude Sonnet/4"), "tau/claude-sonnet-4");
        assert_eq!(managed_name("!!!"), "tau/model");
        assert!(managed_name(&"x".repeat(200)).len() <= 68);
    }
    #[test]
    fn topologies_initialize_without_touching_user_branches() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        let grouped =
            GitWorkspace::initialize(dir.path(), GitTopology::Grouped, vec![child.clone()])
                .unwrap();
        assert!(child.join(".git").exists());
        assert!(grouped.manifest.tau_metadata.join("HEAD").exists());
        let plain = tempfile::tempdir().unwrap();
        let non_git = GitWorkspace::initialize(plain.path(), GitTopology::NonGit, vec![]).unwrap();
        assert!(!plain.path().join(".git").exists());
        assert!(non_git.manifest.tau_metadata.join("HEAD").exists());
    }
}
