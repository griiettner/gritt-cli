//! Native tools (ADR-009): workspace-bounded file read and write, and shell
//! execution with tracked child processes. Every call is resolved to a
//! [`Resource`] first so the policy engine can gate it; nothing here runs
//! without that gate in the agent loop.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use gritt_core::tool::{native, ToolCall, ToolDefinition, ToolResult};
use gritt_core::{Error, Result};
use gritt_provider::CancellationToken;
use tokio::io::AsyncReadExt;

use crate::policy::Resource;

/// Longest tool output returned to the model, in bytes.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// The canonical workspace root every path is resolved against.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.canonicalize().map_err(|error| {
            Error::config(format!(
                "workspace {} is not usable: {error}",
                path.display()
            ))
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves `input` to an absolute path inside the workspace. Rejects
    /// `..` climbing above the root, absolute paths elsewhere, and symlink
    /// escapes by canonicalizing the longest existing prefix.
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let mut normalized = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(outside(input));
                    }
                }
                Component::CurDir => {}
                other => normalized.push(other.as_os_str()),
            }
        }
        if !normalized.starts_with(&self.root) {
            return Err(outside(input));
        }
        // Canonicalize the deepest existing ancestor to catch symlinks.
        let mut existing = normalized.clone();
        let mut tail = Vec::new();
        while !existing.exists() {
            match existing.file_name() {
                Some(name) => {
                    tail.push(name.to_owned());
                    existing.pop();
                }
                None => return Err(outside(input)),
            }
        }
        let mut real = existing
            .canonicalize()
            .map_err(|error| Error::config(format!("cannot resolve `{input}`: {error}")))?;
        if !real.starts_with(&self.root) {
            return Err(outside(input));
        }
        for name in tail.into_iter().rev() {
            real.push(name);
        }
        Ok(real)
    }
}

fn outside(input: &str) -> Error {
    Error::config(format!("path `{input}` is outside the workspace"))
}

/// Pids of children the harness started, so cancellation can stop them.
#[derive(Default)]
pub struct ProcessRegistry {
    pids: Mutex<HashSet<u32>>,
}

impl ProcessRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn track(&self, pid: u32) {
        self.pids.lock().expect("process registry").insert(pid);
    }

    pub fn untrack(&self, pid: u32) {
        self.pids.lock().expect("process registry").remove(&pid);
    }

    pub fn tracked(&self) -> Vec<u32> {
        self.pids
            .lock()
            .expect("process registry")
            .iter()
            .copied()
            .collect()
    }

    /// Kills every tracked child and its process group.
    pub async fn kill_all(&self) {
        for pid in self.tracked() {
            kill_tree(pid).await;
            self.untrack(pid);
        }
    }
}

/// Kills the process group (Unix) or the process tree (Windows) rooted at
/// `pid`. Uses the platform's own tool so no extra dependency is needed.
pub async fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = tokio::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = tokio::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub cancelled: bool,
}

/// The credential-name rule lives in `gritt-core` so the connector crate
/// applies the same one to the environment its agents inherit.
pub use gritt_core::secret::{is_secret_env_name, SECRET_ENV_MARKERS};

/// The names in the current environment a shell child must not inherit.
pub fn secret_env_names(blocked: &[String]) -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_secret_env_name(&name.to_string_lossy(), blocked))
        .collect()
}

fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.process_group(0);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
}

fn truncate(text: &mut String) {
    if text.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[output truncated]");
    }
}

/// Runs `command` in the workspace. The child is tracked until it exits;
/// cancellation kills its process group and reports `cancelled`.
pub async fn run_shell(
    workspace: &Workspace,
    command: &str,
    registry: &ProcessRegistry,
    cancel: &CancellationToken,
    blocked_env: &[String],
) -> Result<ShellOutput> {
    let mut command = shell_command(command);
    for name in secret_env_names(blocked_env) {
        command.env_remove(name);
    }
    let mut child = command
        .current_dir(workspace.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| Error::config(format!("cannot start shell: {error}")))?;
    let pid = child.id().unwrap_or_default();
    registry.track(pid);
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    // Both pipes drain concurrently: a child that fills stderr before it
    // closes stdout would otherwise block forever on a full pipe.
    let read = async {
        let drain_stdout = async {
            if let Some(pipe) = stdout_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut stdout).await;
            }
        };
        let drain_stderr = async {
            if let Some(pipe) = stderr_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut stderr).await;
            }
        };
        tokio::join!(drain_stdout, drain_stderr);
        child.wait().await
    };
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        status = read => Some(status),
    };
    let result = match outcome {
        Some(status) => {
            registry.untrack(pid);
            let status = status.map_err(|error| Error::config(format!("shell failed: {error}")))?;
            let mut out = String::from_utf8_lossy(&stdout).into_owned();
            let mut err = String::from_utf8_lossy(&stderr).into_owned();
            truncate(&mut out);
            truncate(&mut err);
            ShellOutput {
                status: status.code(),
                stdout: out,
                stderr: err,
                cancelled: false,
            }
        }
        None => {
            kill_tree(pid).await;
            registry.untrack(pid);
            ShellOutput {
                status: None,
                stdout: String::new(),
                stderr: String::new(),
                cancelled: true,
            }
        }
    };
    Ok(result)
}

/// Unified diff between the current file content (empty when the file does
/// not exist) and the proposed content, for the approval view.
pub fn unified_diff(path_label: &str, old: &str, new: &str) -> String {
    let patch = diffy::create_patch(old, new);
    let mut text = patch.to_string();
    // diffy labels both sides "original"/"modified"; name the file instead.
    text = text.replacen("--- original", &format!("--- a/{path_label}"), 1);
    text = text.replacen("+++ modified", &format!("+++ b/{path_label}"), 1);
    text
}

/// The three first-version tools and their process registry.
pub struct NativeTools {
    workspace: Workspace,
    registry: Arc<ProcessRegistry>,
    blocked_env: Vec<String>,
}

impl NativeTools {
    pub fn new(workspace: Workspace, registry: Arc<ProcessRegistry>) -> Self {
        Self {
            workspace,
            registry,
            blocked_env: Vec::new(),
        }
    }

    /// Names of environment variables (the configured profile key
    /// variables) that shell children must not inherit, on top of the
    /// suffix rule.
    pub fn with_blocked_env(mut self, names: Vec<String>) -> Self {
        self.blocked_env = names;
        self
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn registry(&self) -> &Arc<ProcessRegistry> {
        &self.registry
    }

    pub fn definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: native::FILE_READ.into(),
                description: "Read a UTF-8 text file inside the workspace.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: native::FILE_WRITE.into(),
                description: "Write a UTF-8 text file inside the workspace, replacing its content. The user reviews a diff first.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root" },
                        "content": { "type": "string", "description": "Complete new file content" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: native::SHELL.into(),
                description: "Run a shell command in the workspace root and return its output. Requires approval.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The command line to run" }
                    },
                    "required": ["command"]
                }),
            },
        ]
    }

    fn argument<'a>(call: &'a ToolCall, name: &str) -> Result<&'a str> {
        call.arguments
            .get(name)
            .and_then(|value| value.as_str())
            .ok_or_else(|| Error::config(format!("tool `{}` needs a `{name}` argument", call.name)))
    }

    /// The resource a call touches, for policy evaluation. Unknown tools
    /// and bad arguments are errors before any policy question.
    pub fn resource_for(&self, call: &ToolCall) -> Result<Resource> {
        match call.name.as_str() {
            native::FILE_READ | native::FILE_WRITE => {
                let path = Self::argument(call, "path")?;
                Ok(Resource::Path(self.workspace.resolve(path)?))
            }
            native::SHELL => Ok(Resource::Command(
                Self::argument(call, "command")?.to_owned(),
            )),
            other => Err(Error::config(format!("unknown tool `{other}`"))),
        }
    }

    /// The diff a write would apply, for review before approval.
    pub fn preview(&self, call: &ToolCall) -> Result<Option<String>> {
        if call.name != native::FILE_WRITE {
            return Ok(None);
        }
        let label = Self::argument(call, "path")?;
        let path = self.workspace.resolve(label)?;
        let content = Self::argument(call, "content")?;
        // Only a missing file is a new file. Any other failure, including
        // invalid UTF-8, would otherwise show a misleading whole-file diff.
        let current = match std::fs::read_to_string(&path) {
            Ok(current) => current,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(Error::config(format!("cannot preview `{label}`: {error}"))),
        };
        Ok(Some(unified_diff(label, &current, content)))
    }

    /// Executes an already-approved call. The caller has run the policy.
    pub async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolResult {
        let outcome = self.execute_inner(call, cancel).await;
        match outcome {
            Ok(output) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: false,
                output,
            },
            Err(error) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: true,
                output: error.message,
            },
        }
    }

    async fn execute_inner(&self, call: &ToolCall, cancel: &CancellationToken) -> Result<String> {
        match call.name.as_str() {
            native::FILE_READ => {
                let path = self.workspace.resolve(Self::argument(call, "path")?)?;
                let mut text = tokio::fs::read_to_string(&path).await.map_err(|error| {
                    Error::config(format!("cannot read `{}`: {error}", path.display()))
                })?;
                truncate(&mut text);
                Ok(text)
            }
            native::FILE_WRITE => {
                let path = self.workspace.resolve(Self::argument(call, "path")?)?;
                let content = Self::argument(call, "content")?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        Error::config(format!("cannot create `{}`: {error}", parent.display()))
                    })?;
                }
                tokio::fs::write(&path, content).await.map_err(|error| {
                    Error::config(format!("cannot write `{}`: {error}", path.display()))
                })?;
                Ok(format!(
                    "wrote {} bytes to {}",
                    content.len(),
                    path.display()
                ))
            }
            native::SHELL => {
                let command = Self::argument(call, "command")?;
                let output = run_shell(
                    &self.workspace,
                    command,
                    &self.registry,
                    cancel,
                    &self.blocked_env,
                )
                .await?;
                if output.cancelled {
                    return Err(Error::cancelled());
                }
                let mut text = String::new();
                if !output.stdout.is_empty() {
                    text.push_str(&output.stdout);
                }
                if !output.stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str("[stderr]\n");
                    text.push_str(&output.stderr);
                }
                match output.status {
                    Some(0) => Ok(text),
                    Some(code) => Err(Error::config(format!("exit status {code}\n{text}"))),
                    None => Err(Error::config(format!("terminated by signal\n{text}"))),
                }
            }
            other => Err(Error::config(format!("unknown tool `{other}`"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::tool::ToolCallId;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        (dir, ws)
    }

    #[test]
    fn paths_cannot_escape_the_workspace() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve("../etc/passwd").is_err());
        assert!(ws.resolve("a/../../x").is_err());
        assert!(ws.resolve("/etc/passwd").is_err());
        let inside = ws.resolve("a/./b/../c.txt").unwrap();
        assert_eq!(inside, ws.root().join("a/c.txt"));
        let absolute_inside = ws
            .resolve(&ws.root().join("z.txt").to_string_lossy())
            .unwrap();
        assert_eq!(absolute_inside, ws.root().join("z.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let (dir, ws) = workspace();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(ws.resolve("link/secret.txt").is_err());
        assert!(ws.resolve("link").is_err());
    }

    #[test]
    fn diff_names_the_file() {
        let diff = unified_diff("a.txt", "one\n", "one\ntwo\n");
        assert!(diff.contains("--- a/a.txt"));
        assert!(diff.contains("+++ b/a.txt"));
        assert!(diff.contains("+two"));
    }

    #[tokio::test]
    async fn file_tools_round_trip_and_preview() {
        let (_dir, ws) = workspace();
        let tools = NativeTools::new(ws.clone(), ProcessRegistry::new());
        let cancel = CancellationToken::new();
        let write = ToolCall {
            id: ToolCallId("w".into()),
            name: native::FILE_WRITE.into(),
            arguments: serde_json::json!({"path": "notes/a.txt", "content": "hello\n"}),
        };
        let preview = tools.preview(&write).unwrap().unwrap();
        assert!(preview.contains("+hello"));
        let result = tools.execute(&write, &cancel).await;
        assert!(!result.is_error, "{}", result.output);
        let read = ToolCall {
            id: ToolCallId("r".into()),
            name: native::FILE_READ.into(),
            arguments: serde_json::json!({"path": "notes/a.txt"}),
        };
        let result = tools.execute(&read, &cancel).await;
        assert_eq!(result.output, "hello\n");
        let escape = ToolCall {
            id: ToolCallId("e".into()),
            name: native::FILE_READ.into(),
            arguments: serde_json::json!({"path": "../outside"}),
        };
        assert!(tools.resource_for(&escape).is_err());
        let result = tools.execute(&escape, &cancel).await;
        assert!(result.is_error);
        assert!(result.output.contains("outside the workspace"));
    }

    #[tokio::test]
    async fn shell_runs_in_the_workspace() {
        let (_dir, ws) = workspace();
        let registry = ProcessRegistry::new();
        let output = run_shell(
            &ws,
            "echo hi && pwd",
            &registry,
            &CancellationToken::new(),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(output.status, Some(0));
        assert!(output.stdout.starts_with("hi\n"));
        assert!(registry.tracked().is_empty());
    }

    #[test]
    fn preview_propagates_read_errors_other_than_not_found() {
        let (_dir, ws) = workspace();
        let tools = NativeTools::new(ws.clone(), ProcessRegistry::new());
        std::fs::write(ws.root().join("bin.dat"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let call = ToolCall {
            id: ToolCallId("p".into()),
            name: native::FILE_WRITE.into(),
            arguments: serde_json::json!({"path": "bin.dat", "content": "text\n"}),
        };
        let error = tools.preview(&call).unwrap_err();
        assert!(
            error.message.contains("cannot preview"),
            "{}",
            error.message
        );
        let fresh = ToolCall {
            id: ToolCallId("n".into()),
            name: native::FILE_WRITE.into(),
            arguments: serde_json::json!({"path": "new.txt", "content": "text\n"}),
        };
        assert!(tools.preview(&fresh).unwrap().unwrap().contains("+text"));
    }

    #[test]
    fn secret_env_names_follow_the_suffix_and_profile_rules() {
        let blocked = vec!["MY_PROVIDER_CREDENTIAL".to_string()];
        assert!(is_secret_env_name("OPENAI_API_KEY", &blocked));
        assert!(is_secret_env_name("gh_token", &blocked));
        assert!(is_secret_env_name("APP_SECRET", &blocked));
        assert!(is_secret_env_name("AGENT_MEMORY_API_KEY", &blocked));
        assert!(is_secret_env_name("my_provider_credential", &blocked));
        assert!(is_secret_env_name("AWS_SECRET_ACCESS_KEY", &blocked));
        assert!(is_secret_env_name("GITHUB_TOKEN", &blocked));
        assert!(is_secret_env_name("NPM_TOKEN", &blocked));
        assert!(is_secret_env_name("DB_PASSWORD", &blocked));
        assert!(is_secret_env_name("pgpasswd", &blocked));
        assert!(is_secret_env_name("KEYCHAIN_PATH", &blocked));
        assert!(!is_secret_env_name("PATH", &blocked));
        assert!(!is_secret_env_name("HOME", &blocked));
        assert!(!is_secret_env_name("TERM", &blocked));
        assert!(!is_secret_env_name("LC_ALL", &blocked));
        assert!(!is_secret_env_name("CARGO_HOME", &blocked));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_children_do_not_inherit_credentials() {
        let (_dir, ws) = workspace();
        std::env::set_var("GRITT_TEST_SHELL_API_KEY", "shell-env-secret-8841");
        std::env::set_var("GRITT_TEST_PROFILE_CRED", "profile-env-secret-8842");
        std::env::set_var("GRITT_TEST_PLAIN", "plain-value-8843");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "aws-env-secret-8844");
        std::env::set_var("GITHUB_TOKEN", "github-env-secret-8845");
        let registry = ProcessRegistry::new();
        let output = run_shell(
            &ws,
            "env",
            &registry,
            &CancellationToken::new(),
            &["GRITT_TEST_PROFILE_CRED".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(output.status, Some(0));
        assert!(!output.stdout.contains("shell-env-secret-8841"));
        assert!(!output.stdout.contains("profile-env-secret-8842"));
        assert!(!output.stdout.contains("aws-env-secret-8844"));
        assert!(!output.stdout.contains("github-env-secret-8845"));
        assert!(output.stdout.contains("plain-value-8843"));
        assert!(output.stdout.contains("PATH="));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_that_floods_stderr_first_does_not_deadlock() {
        let (_dir, ws) = workspace();
        let registry = ProcessRegistry::new();
        let command = "head -c 300000 /dev/zero | tr '\\0' x >&2; echo done";
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            run_shell(&ws, command, &registry, &CancellationToken::new(), &[]),
        )
        .await
        .expect("shell drained both pipes")
        .unwrap();
        assert_eq!(output.status, Some(0));
        assert_eq!(output.stdout, "done\n");
        assert!(output.stderr.len() >= 300_000 || output.stderr.contains("[output truncated]"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_the_child_process() {
        let (_dir, ws) = workspace();
        let registry = ProcessRegistry::new();
        let cancel = CancellationToken::new();
        let marker = format!("gritt-cancel-test-{}", std::process::id());
        let command = format!("sleep 30 # {marker}");
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let output = run_shell(&ws, &command, &registry, &cancel, &[])
            .await
            .unwrap();
        assert!(output.cancelled);
        assert!(registry.tracked().is_empty());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let ps = std::process::Command::new("ps")
            .args(["-eo", "args"])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&ps.stdout);
        assert!(
            !listing
                .lines()
                .any(|line| line.contains(&marker) && !line.contains("ps ")),
            "child survived cancellation: {listing}"
        );
    }
}
