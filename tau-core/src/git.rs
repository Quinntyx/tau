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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitFileStatus {
    pub path: String,
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub revision: String,
    pub files: Vec<GitFileStatus>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitFile {
    pub path: String,
    pub content: String,
    pub diff: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
}
impl GitWorkspace {
    /// Open an existing project without creating metadata or changing it.
    ///
    /// Daemon requests use this constructor so repository operations remain in
    /// the core service rather than leaking filesystem/Git access into a UI or
    /// transport crate.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = std::fs::canonicalize(root.into()).context("canonicalizing project root")?;
        if !root.is_dir() {
            bail!("project root is not a directory")
        }
        Ok(Self {
            manifest: RepositoryManifest {
                root: root.clone(),
                topology: GitTopology::Direct,
                children: Vec::new(),
                tau_metadata: root.join(".tau/metadata.git"),
            },
        })
    }

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
        ensure_clean(repository, "creating managed worktree")?;
        let mut branch = managed_name(model);
        let mut folder = self.manifest.root.join(&branch);
        // Keep the human-readable model name, but disambiguate an existing
        // folder that was created for a different full model identifier.
        let marker = self.manifest.tau_metadata.join("model-map").join(&branch);
        if (folder.exists() || branch_exists(repository, &branch))
            && (!marker.is_file()
                || std::fs::read_to_string(&marker).ok().as_deref() != Some(model))
        {
            branch = format!("{branch}-{}", short_model_hash(model));
            folder = self.manifest.root.join(&branch);
        }
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
            std::fs::create_dir_all(marker.parent().expect("model marker parent"))?;
            std::fs::write(&marker, model)
                .with_context(|| format!("writing model marker in {}", folder.display()))?;
        }
        Ok(ManagedWorktree {
            branch,
            path: folder,
            repository: repository.to_path_buf(),
        })
    }
    fn project(&self, project: &Path) -> Result<PathBuf> {
        let root = std::fs::canonicalize(&self.manifest.root).with_context(|| {
            format!(
                "canonicalizing project root {}",
                self.manifest.root.display()
            )
        })?;
        let path = if project.is_absolute() {
            project.to_path_buf()
        } else {
            root.join(project)
        };
        let resolved = std::fs::canonicalize(&path)
            .with_context(|| format!("canonicalizing project path {}", path.display()))?;
        if resolved != root && !resolved.starts_with(&root) {
            bail!("project path is outside workspace")
        }
        Ok(resolved)
    }
    fn file_path(&self, project: &Path, path: &Path) -> Result<(PathBuf, String)> {
        if path.is_absolute() {
            bail!("file path must be relative")
        }
        let project = self.project(project)?;
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("file path traversal is not allowed")
        }
        let full = project.join(path);
        let canonical = if full.exists() {
            std::fs::canonicalize(&full)?
        } else {
            let parent = full.parent().context("file has no parent")?;
            std::fs::canonicalize(parent)?.join(full.file_name().context("empty file path")?)
        };
        if canonical != project && !canonical.starts_with(&project) {
            bail!("file path is outside project")
        }
        Ok((canonical, path.to_string_lossy().into_owned()))
    }
    pub fn status(&self, project: &Path) -> Result<GitStatus> {
        let project = self.project(project)?;
        let branch =
            String::from_utf8_lossy(&git_output(&project, &["branch", "--show-current"])?.stdout)
                .trim()
                .into();
        let revision =
            String::from_utf8_lossy(&git_output(&project, &["rev-parse", "HEAD"])?.stdout)
                .trim()
                .into();
        let raw = git_output(&project, &["status", "--porcelain=v1", "-z"])?.stdout;
        let files = raw
            .split(|b| *b == 0)
            .filter(|x| !x.is_empty())
            .filter_map(|x| {
                let text = String::from_utf8_lossy(x);
                let mut chars = text.chars();
                let staged = chars.next().map(|c| c != ' ' && c != '?').unwrap_or(false);
                let modified = chars.next().map(|c| c != ' ' && c != '?').unwrap_or(false);
                let untracked = text.starts_with("??");
                let path = text.get(3..).unwrap_or("").to_owned();
                if path.is_empty() {
                    None
                } else {
                    Some(GitFileStatus {
                        path,
                        staged,
                        modified,
                        untracked,
                    })
                }
            })
            .collect();
        Ok(GitStatus {
            branch,
            revision,
            files,
        })
    }
    pub fn file(&self, project: &Path, path: &Path) -> Result<GitFile> {
        let (full, display) = self.file_path(project, path)?;
        let repo = self.project(project)?;
        let content = std::fs::read_to_string(&full)
            .with_context(|| format!("reading {}", full.display()))?;
        let diff =
            String::from_utf8_lossy(&git_output(&repo, &["diff", "HEAD", "--", &display])?.stdout)
                .into_owned();
        Ok(GitFile {
            path: display,
            content,
            diff,
        })
    }
    pub fn stage(&self, project: &Path, path: &Path) -> Result<()> {
        let (_, p) = self.file_path(project, path)?;
        let repo = self.project(project)?;
        git(&repo, &["add", "--", &p])?;
        Ok(())
    }
    pub fn unstage(&self, project: &Path, path: &Path) -> Result<()> {
        let (_, p) = self.file_path(project, path)?;
        let repo = self.project(project)?;
        git(&repo, &["restore", "--staged", "--", &p])?;
        Ok(())
    }
    pub fn revert(&self, project: &Path, path: &Path, confirmed: bool) -> Result<()> {
        if !confirmed {
            bail!("revert requires explicit confirmation")
        }
        let (_, p) = self.file_path(project, path)?;
        let repo = self.project(project)?;
        git(&repo, &["restore", "--source=HEAD", "--staged", "--", &p])?;
        git(&repo, &["restore", "--", &p])?;
        Ok(())
    }
    pub fn branches(&self, project: &Path) -> Result<Vec<GitBranch>> {
        let repo = self.project(project)?;
        let current =
            String::from_utf8_lossy(&git_output(&repo, &["branch", "--show-current"])?.stdout)
                .trim()
                .to_owned();
        let output = git_output(
            &repo,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )?;
        let names = String::from_utf8_lossy(&output.stdout);
        Ok(names
            .lines()
            .map(|name| GitBranch {
                name: name.into(),
                current: name == current,
            })
            .collect())
    }
    pub fn create_branch(&self, project: &Path, name: &str) -> Result<()> {
        validate_branch(name)?;
        let repo = self.project(project)?;
        git(&repo, &["branch", name])?;
        Ok(())
    }
    pub fn switch_branch(&self, project: &Path, name: &str) -> Result<()> {
        validate_branch(name)?;
        let repo = self.project(project)?;
        ensure_clean(&repo, "switching branch")?;
        git(&repo, &["switch", name])?;
        Ok(())
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
        if !source.starts_with("tau/") || !target.starts_with("tau/") {
            bail!("integration is restricted to tau-managed branches");
        }
        ensure_clean(repository, "integrating managed worktree")?;
        let preview = self.preview_integration(repository, source, target)?;
        if preview.conflicts && !force {
            bail!("integration has conflicts; resolve or force explicitly")
        }
        let current =
            String::from_utf8_lossy(&git_output(repository, &["branch", "--show-current"])?.stdout)
                .trim()
                .to_owned();
        if current != target {
            git(repository, &["checkout", target])?;
        }
        let merged = git(repository, &["merge", "--no-ff", source]);
        if merged.is_err() {
            let _ = git(repository, &["merge", "--abort"]);
        }
        if current != target {
            let _ = git(repository, &["checkout", &current]);
        }
        merged?;
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
fn short_model_hash(model: &str) -> String {
    use sha1::{Digest, Sha1};
    format!("{:x}", Sha1::digest(model.as_bytes()))[..10].to_owned()
}
fn ensure_clean(repository: &Path, operation: &str) -> Result<()> {
    let status = git_output(repository, &["status", "--porcelain"])?;
    if !status.stdout.is_empty() {
        bail!("refusing {operation}: repository has uncommitted changes")
    }
    Ok(())
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
fn validate_branch(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name.contains(' ')
        || name.contains('~')
        || name.contains('^')
        || name.contains(':')
        || name.contains('\\')
        || name.ends_with('/')
    {
        bail!("invalid branch name")
    }
    Ok(())
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
    fn branch_names_reject_git_argument_injection_and_ambiguous_names() {
        assert!(validate_branch("feature/topic").is_ok());
        assert!(validate_branch("-c").is_err());
        assert!(validate_branch("feature..topic").is_err());
        assert!(validate_branch("feature\\topic").is_err());
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
