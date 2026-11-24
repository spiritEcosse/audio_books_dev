use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::db::NewScene;
use crate::error::AppError;

const FALLBACK_SHELL: &str = "/bin/zsh";

const SYSTEM_PROMPT: &str = "You split a story into scenes for an illustrated audiobook video. \
Keep the original text intact and in order: each scene's \"text\" is a consecutive passage \
meant to be narrated aloud (roughly 20-60 seconds). \"image_prompt\" is a vivid English \
description of one still image for that scene. Reply with ONLY a JSON array of objects \
with keys \"text\" and \"image_prompt\", no commentary.";

/// Headless, tool-less and isolated from the user's global settings, hooks, plugins and MCP
/// servers: without this a tiny request loads ~90k tokens of unrelated context.
const HEADLESS_ARGS: [&str; 10] = [
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
];

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

/// Splits a story with the Claude Code CLI in headless mode (`claude -p`),
/// so it runs on the user's Claude login instead of an API key. Blocking.
pub fn split_into_scenes(cwd: &Path, story_text: &str) -> Result<Vec<NewScene>, AppError> {
    let mut child = Command::new(resolve_claude()?)
        .args(HEADLESS_ARGS)
        .arg("--disable-slash-commands")
        .args(["--system-prompt", SYSTEM_PROMPT])
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
        .write_all(story_text.as_bytes())
        .map_err(|e| AppError::new(format!("cannot send story to claude: {e}")))?;

    let output = child
        .wait_with_output()
        .map_err(|e| AppError::new(format!("claude failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::new(format!("claude exited with {}: {}", output.status, stderr.trim())));
    }
    parse_scenes(&extract_result(&output.stdout)?)
}

/// Pulls the model's reply out of `claude -p --output-format json` output.
fn extract_result(stdout: &[u8]) -> Result<String, AppError> {
    let payload: Value = serde_json::from_slice(stdout)
        .map_err(|e| AppError::new(format!("unexpected claude output: {e}")))?;
    if payload["is_error"].as_bool().unwrap_or(false) {
        let message = payload["result"].as_str().unwrap_or("unknown error");
        return Err(AppError::new(format!("claude error: {message}")));
    }
    payload["result"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| AppError::new("claude output has no result"))
}

/// Extracts the JSON array from a model reply, tolerating code fences or stray prose.
pub fn parse_scenes(reply: &str) -> Result<Vec<NewScene>, AppError> {
    let start = reply.find('[');
    let end = reply.rfind(']');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(AppError::new("Claude reply contains no JSON array"));
    };
    let scenes: Vec<NewScene> = serde_json::from_str(&reply[start..=end])
        .map_err(|err| AppError::new(format!("invalid scenes JSON: {err}")))?;
    if scenes.is_empty() {
        return Err(AppError::new("Claude returned zero scenes"));
    }
    Ok(scenes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_array() {
        let scenes = parse_scenes(r#"[{"text":"Hi","image_prompt":"a cat"}]"#).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].image_prompt, "a cat");
    }

    #[test]
    fn parses_array_wrapped_in_code_fence() {
        let reply = "```json\n[{\"text\":\"A\",\"image_prompt\":\"b\"}]\n```";
        assert_eq!(parse_scenes(reply).unwrap().len(), 1);
    }

    #[test]
    fn extracts_result_from_cli_json() {
        let out = br#"{"type":"result","is_error":false,"result":"[]"}"#;
        assert_eq!(extract_result(out).unwrap(), "[]");
    }

    #[test]
    fn surfaces_cli_error_flag() {
        let out = br#"{"is_error":true,"result":"Not logged in"}"#;
        assert!(extract_result(out).unwrap_err().to_string().contains("Not logged in"));
    }

    #[test]
    fn rejects_non_json_cli_output() {
        assert!(extract_result(b"garbage").is_err());
    }

    #[test]
    fn rejects_reply_without_array() {
        assert!(parse_scenes("sorry, no").is_err());
    }

    #[test]
    fn rejects_empty_array() {
        assert!(parse_scenes("[]").is_err());
    }

    #[test]
    fn rejects_malformed_scene_objects() {
        assert!(parse_scenes(r#"[{"text":"only text"}]"#).is_err());
    }
}
