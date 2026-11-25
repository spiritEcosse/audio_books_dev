use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::AppError;

const FALLBACK_SHELL: &str = "/bin/zsh";

/// Headless, tool-less and isolated from the user's global settings, hooks, plugins and MCP
/// servers: without this a tiny request loads ~90k tokens of unrelated context.
const HEADLESS_ARGS: [&str; 11] = [
    "-p",
    "--output-format",
    "json",
    "--tools",
    "",
    "--setting-sources",
    "",
    "--strict-mcp-config",
    "--mcp-config",
    r#"{"mcpServers":{}}"#,
    "--disable-slash-commands",
];

pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

/// Models the user can pick. Only these ids ever reach the CLI's `--model` flag.
pub const MODELS: [ModelOption; 3] = [
    ModelOption { id: "claude-sonnet-5-5", label: "Sonnet 5.5 (баланс)" },
    ModelOption { id: "claude-opus-5-5", label: "Opus 5.5 (самая сильная)" },
    ModelOption { id: "claude-haiku-5-5", label: "Haiku 5.5 (быстрая и дешёвая)" },
];

pub const DEFAULT_MODEL: &str = MODELS[0].id;

pub fn is_known_model(id: &str) -> bool {
    MODELS.iter().any(|m| m.id == id)
}

pub struct CliReply {
    pub text: String,
    pub session_id: Option<String>,
}

/// Finds the `claude` binary. A Finder-launched app has a minimal PATH, so ask a
/// login shell, which sees the user's real one.
fn resolve_claude() -> Result<PathBuf, AppError> {
    if let Ok(path) = std::env::var("CLAUDE_BIN") {
        return Ok(PathBuf::from(path));
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| FALLBACK_SHELL.into());
    let output = Command::new(shell)
        .args(["-l", "-i", "-c", "command -v claude"])
        .output()
        .map_err(|e| AppError::new(format!("cannot run login shell: {e}")))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| line.starts_with('/'))
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("`claude` not found. Install Claude Code or set CLAUDE_BIN"))
}

/// Runs one headless Claude Code turn (`claude -p`), optionally continuing a saved session.
/// The prompt goes through stdin. Blocking.
pub fn run(
    cwd: &Path,
    model: &str,
    system_prompt: &str,
    prompt: &str,
    resume: Option<&str>,
) -> Result<CliReply, AppError> {
    let mut command = Command::new(resolve_claude()?);
    command
        .args(HEADLESS_ARGS)
        .args(["--model", model, "--system-prompt", system_prompt]);
    if let Some(session_id) = resume {
        command.args(["--resume", session_id]);
    }
    exec(command, cwd, prompt)
}

/// Spawns the prepared command, feeds it the prompt through stdin and parses the JSON reply.
fn exec(mut command: Command, cwd: &Path, prompt: &str) -> Result<CliReply, AppError> {
    let mut child = command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(format!("cannot start claude: {e}")))?;

    child
        .stdin
        .take()
        .ok_or_else(|| AppError::new("claude stdin unavailable"))?
        .write_all(prompt.as_bytes())
        .map_err(|e| AppError::new(format!("cannot send prompt to claude: {e}")))?;

    let output = child
        .wait_with_output()
        .map_err(|e| AppError::new(format!("claude failed: {e}")))?;
    if !output.status.success() {
        // The CLI prints some failures (e.g. unknown session) to stdout, others to stderr.
        let details = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(AppError::new(format!("claude exited with {}: {}", output.status, details.trim())));
    }
    parse_cli_output(&output.stdout)
}

/// A headless turn that may call claude.ai connector tools. User settings (hooks, plugins) stay off.
/// Do not add `--tools`: combined with `--setting-sources` it stops connector tools from loading.
const CONNECTOR_ARGS: [&str; 6] =
    ["-p", "--output-format", "json", "--setting-sources", "", "--disable-slash-commands"];

/// Runs one headless turn that may use the connector tools matching `allowed_tools`
/// (e.g. `mcp__claude_ai_ElevenLabs`); everything else stays unapproved and is denied. Blocking.
pub fn run_connector(
    cwd: &Path,
    model: &str,
    system_prompt: &str,
    prompt: &str,
    allowed_tools: &str,
    max_turns: u32,
) -> Result<CliReply, AppError> {
    if !is_known_model(model) {
        return Err(AppError::new(format!("unknown model: {model}")));
    }
    let mut command = Command::new(resolve_claude()?);
    command.args(CONNECTOR_ARGS).args([
        "--model",
        model,
        "--system-prompt",
        system_prompt,
        "--allowedTools",
        allowed_tools,
        "--max-turns",
        &max_turns.to_string(),
    ]);
    exec(command, cwd, prompt)
}

const SESSION_MISSING_MARKER: &str = "No conversation found";

/// True when a `--resume` failed only because the saved session no longer exists.
pub fn is_session_missing(err: &AppError) -> bool {
    err.to_string().contains(SESSION_MISSING_MARKER)
}

/// Pulls the reply text and session id out of `claude -p --output-format json` output.
pub fn parse_cli_output(stdout: &[u8]) -> Result<CliReply, AppError> {
    let payload: Value = serde_json::from_slice(stdout)
        .map_err(|e| AppError::new(format!("unexpected claude output: {e}")))?;
    if payload["is_error"].as_bool().unwrap_or(false) {
        let message = payload["result"].as_str().unwrap_or("unknown error");
        return Err(AppError::new(format!("claude error: {message}")));
    }
    let text = payload["result"]
        .as_str()
        .ok_or_else(|| AppError::new("claude output has no result"))?
        .to_owned();
    let session_id = payload["session_id"].as_str().map(str::to_owned);
    Ok(CliReply { text, session_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_and_session_id() {
        let out = br#"{"is_error":false,"result":"hi","session_id":"abc"}"#;
        let reply = parse_cli_output(out).unwrap();
        assert_eq!(reply.text, "hi");
        assert_eq!(reply.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn surfaces_cli_error_flag() {
        let out = br#"{"is_error":true,"result":"Not logged in"}"#;
        assert!(parse_cli_output(out).err().unwrap().to_string().contains("Not logged in"));
    }

    #[test]
    fn only_listed_models_are_known() {
        assert!(is_known_model(DEFAULT_MODEL));
        assert!(!is_known_model("--dangerous-flag"));
    }

    #[test]
    fn detects_missing_session_error() {
        let missing = AppError::new("claude exited with 1: No conversation found with session ID: x");
        assert!(is_session_missing(&missing));
        assert!(!is_session_missing(&AppError::new("claude exited with 1: network unreachable")));
    }

    #[test]
    fn rejects_non_json_cli_output() {
        assert!(parse_cli_output(b"garbage").is_err());
    }
}
