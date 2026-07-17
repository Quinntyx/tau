use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::error::{ToolError, io};
use super::registry::{Tool, ToolDescriptor};
use super::snapshot::SnapshotStore;
use super::types::ToolContext;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashInput {
    pub command: String,
    pub workdir: Option<PathBuf>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum CommandClass {
    ReadOnly,
    FilesystemMutation,
    ChangesDirectory,
    PotentialMutation,
}

#[derive(Debug, Clone, Serialize)]
pub struct BashOutput {
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    pub classification: CommandClass,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct BashTool;

impl Tool for BashTool {
    type Input = BashInput;
    type Output = BashOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "bash".into(),
            description: "Execute a platform shell command, classify likely mutations, and return bounded output.".into(),
        }
    }

    fn execute(&self, input: BashInput, context: &ToolContext) -> Result<BashOutput, ToolError> {
        let cwd = context
            .policy
            .resolve(
                &context.cwd,
                input
                    .workdir
                    .as_deref()
                    .unwrap_or(std::path::Path::new(".")),
                "bash",
            )?
            .path;
        let classification = classify_command(&input.command);
        let snapshot =
            SnapshotStore::for_cwd(&context.cwd).capture_paths(std::slice::from_ref(&cwd))?;
        let timeout = input.timeout.unwrap_or(context.limits.bash_timeout_seconds);
        let mut command = platform_command(&input.command);
        command
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| io("spawn bash", &cwd, e))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_thread = thread::spawn(move || read_pipe(stdout));
        let stderr_thread = thread::spawn(move || read_pipe(stderr));
        let started = Instant::now();
        let mut timed_out = false;
        let status = loop {
            match child.try_wait().map_err(|e| io("wait bash", &cwd, e))? {
                Some(status) => break Some(status),
                None if started.elapsed() >= Duration::from_secs(timeout) => {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().ok();
                }
                None => thread::sleep(Duration::from_millis(10)),
            }
        };
        let stdout = stdout_thread.join().unwrap_or_else(|_| Vec::new());
        let stderr = stderr_thread.join().unwrap_or_else(|_| Vec::new());
        let (stdout, stdout_truncated) = truncate_output(
            &stdout,
            context.limits.bash_lines,
            context.limits.bash_bytes,
        );
        let (stderr, stderr_truncated) = truncate_output(
            &stderr,
            context.limits.bash_lines,
            context.limits.bash_bytes,
        );
        Ok(BashOutput {
            command: input.command,
            cwd,
            exit_code: status.and_then(|status| status.code()),
            stdout,
            stderr,
            timed_out,
            truncated: stdout_truncated || stderr_truncated,
            classification,
            snapshot_id: snapshot.id,
        })
    }

    fn render(&self, output: &BashOutput) -> String {
        format!(
            "exit={:?} class={:?} timeout={}\nstdout:\n{}\nstderr:\n{}\nsnapshot={}",
            output.exit_code,
            output.classification,
            output.timed_out,
            output.stdout,
            output.stderr,
            output.snapshot_id
        )
    }
}

#[cfg(unix)]
fn platform_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn platform_command(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}

fn read_pipe(pipe: Option<impl Read>) -> Vec<u8> {
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    bytes
}

fn truncate_output(bytes: &[u8], max_lines: usize, max_bytes: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let lines = text.lines().collect::<Vec<_>>();
    let too_many_lines = lines.len() > max_lines;
    let mut selected = if too_many_lines {
        let head = max_lines / 2;
        let tail = max_lines.saturating_sub(head);
        lines[..head]
            .iter()
            .chain(lines[lines.len().saturating_sub(tail)..].iter())
            .copied()
            .collect::<Vec<_>>()
    } else {
        lines
    };
    let mut result = selected.join("\n");
    let mut truncated = too_many_lines;
    if result.len() > max_bytes {
        let head_bytes = max_bytes / 2;
        let tail_bytes = max_bytes.saturating_sub(head_bytes);
        let head = result.chars().take(head_bytes).collect::<String>();
        let tail = result
            .chars()
            .rev()
            .take(tail_bytes)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        result = format!("{head}\n... output truncated ...\n{tail}");
        truncated = true;
    }
    selected.clear();
    (result, truncated)
}

pub fn classify_command(command: &str) -> CommandClass {
    let lower = command.to_ascii_lowercase();
    if ["sed", "perl", "python", "python3", "ruby", "node", "awk"]
        .iter()
        .any(|name| {
            lower
                .split_whitespace()
                .any(|token| token == *name || token.ends_with(&format!("/{name}")))
        })
    {
        return CommandClass::PotentialMutation;
    }
    if lower.split_whitespace().any(|token| {
        matches!(
            token.trim_matches(
                |character: char| !character.is_ascii_alphanumeric() && character != '-'
            ),
            "rm" | "cp"
                | "mv"
                | "mkdir"
                | "touch"
                | "chmod"
                | "chown"
                | "tee"
                | "dd"
                | "install"
                | "truncate"
        )
    }) {
        return CommandClass::FilesystemMutation;
    }
    if lower
        .split_whitespace()
        .any(|token| matches!(token, "cd" | "pushd" | "popd"))
    {
        return CommandClass::ChangesDirectory;
    }
    CommandClass::ReadOnly
}
