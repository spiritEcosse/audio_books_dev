//! Pieces shared by the narration and image queues: reading Claude's JSON answers, trusted downloads and
//! uploads, versioned file names and the `scene_assets` bookkeeping.

use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::error::AppError;
use crate::media::{KIND_AUDIO, STATUS_REQUESTED};

pub const CONNECTOR_TOOLS: &str = "mcp__claude_ai_ElevenLabs";
/// Cheap model: it only forwards text to tools and relays their answers.
pub const MEDIA_MODEL: &str = "claude-haiku-5-5";
const TRUSTED_HOST: &str = "elevenlabsusercontent.com";
const TRANSFER_TIMEOUT_SECS: &str = "120";
const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const DEFAULT_IMAGE_EXTENSION: &str = "webp";

/// A queued request that generation has not started for yet.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedRow {
    pub position: i64,
    pub source: String,
    pub model_id: String,
    pub voice_id: Option<String>,
    pub estimate_credits: Option<f64>,
}

/// What Claude reported for one scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub position: i64,
    pub url: Option<String>,
    pub credits: Option<f64>,
    pub prompt: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub path: String,
    pub credits: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemResult {
    pub position: i64,
    pub saved: Result<Saved, String>,
}

pub fn queued_rows(conn: &Connection, story_id: i64, kind: &str) -> Result<Vec<QueuedRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT position, source, model_id, voice_id, estimate_credits FROM scene_assets
         WHERE story_id = ?1 AND kind = ?2 AND status = ?3 AND started_at IS NULL ORDER BY position",
    )?;
    let rows = stmt.query_map(params![story_id, kind, STATUS_REQUESTED], |row| {
        Ok(QueuedRow {
            position: row.get(0)?,
            source: row.get(1)?,
            model_id: row.get(2)?,
            voice_id: row.get(3)?,
            estimate_credits: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn items_prompt(task: &str, items: Vec<Value>) -> String {
    format!("{task}:\n{}", Value::Array(items))
}

pub fn extract_json(text: &str) -> Result<Value, AppError> {
    match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => serde_json::from_str(&text[start..=end])
            .map_err(|e| AppError::new(format!("unreadable answer from Claude: {e}"))),
        _ => Err(AppError::new("Claude's answer contained no JSON")),
    }
}

fn error_messages(json: &Value) -> String {
    json["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["message"].as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn parse_estimates(text: &str) -> Result<Vec<(i64, f64)>, AppError> {
    let json = extract_json(text)?;
    let list = json["estimates"].as_array().cloned().unwrap_or_default();
    let estimates: Vec<(i64, f64)> = list
        .iter()
        .filter_map(|e| {
            let credits = e["credits"].as_f64().filter(|c| c.is_finite() && *c >= 0.0)?;
            Some((e["position"].as_i64()?, credits))
        })
        .collect();
    if estimates.is_empty() {
        let reasons = error_messages(&json);
        let suffix = if reasons.is_empty() { String::new() } else { format!(": {reasons}") };
        return Err(AppError::new(format!("no price returned{suffix}")));
    }
    Ok(estimates)
}

pub fn parse_outcomes(text: &str) -> Result<Vec<Outcome>, AppError> {
    let json = extract_json(text)?;
    let mut outcomes = Vec::new();
    for r in json["results"].as_array().into_iter().flatten() {
        let Some(position) = r["position"].as_i64() else { continue };
        outcomes.push(Outcome {
            position,
            url: r["url"].as_str().map(str::to_owned),
            credits: r["credits"].as_f64(),
            prompt: r["prompt"].as_str().map(str::to_owned),
            error: None,
        });
    }
    for e in json["errors"].as_array().into_iter().flatten() {
        let Some(position) = e["position"].as_i64() else { continue };
        let message = e["message"].as_str().unwrap_or("unknown error");
        outcomes.push(Outcome { position, url: None, credits: None, prompt: None, error: Some(message.to_owned()) });
    }
    Ok(outcomes)
}

/// The result for a scene: a successful one if Claude reported any, else its error.
pub fn outcome_for(outcomes: &[Outcome], position: i64) -> Option<&Outcome> {
    let own = || outcomes.iter().filter(move |o| o.position == position);
    own().find(|o| o.url.is_some()).or_else(|| own().next())
}

/// Only https URLs on ElevenLabs' content host are fetched or written to: URLs come from model output.
pub fn is_trusted_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.contains('@') && !host.contains(':') && (host == TRUSTED_HOST || host.ends_with(&format!(".{TRUSTED_HOST}")))
}

fn file_extension(url: &str, kind: &str) -> String {
    if kind == KIND_AUDIO {
        return "mp3".into();
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or_else(|| DEFAULT_IMAGE_EXTENSION.to_owned())
}

/// `scene-<position>-<kind>-v<N>.<ext>`: a new name each time, the webview caches by URL.
pub fn next_versioned_path(dir: &Path, position: i64, kind: &str, extension: &str) -> PathBuf {
    let prefix = format!("scene-{position}-{kind}-v");
    let newest = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let digits: String = name.strip_prefix(&prefix)?.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0);
    dir.join(format!("{prefix}{}.{extension}", newest + 1))
}

fn curl_failed(what: &str, status: std::process::ExitStatus) -> String {
    format!("{what} failed ({status})")
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    if !is_trusted_url(url) {
        return Err("refused to download from an untrusted URL".into());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut partial = dest.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", TRANSFER_TIMEOUT_SECS, "-o"])
        .arg(&partial)
        .args(["--", url])
        .status()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let size = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if !status.success() || size == 0 {
        let _ = std::fs::remove_file(&partial);
        return Err(curl_failed("download", status));
    }
    std::fs::rename(&partial, dest).map_err(|e| format!("cannot save {}: {e}", dest.display()))
}

/// PUTs a local file to an upload URL that Claude obtained from the connector.
pub fn upload(url: &str, file: &Path, mime: &str) -> Result<(), String> {
    if !is_trusted_url(url) {
        return Err("refused to upload to an untrusted URL".into());
    }
    let status = Command::new("curl")
        .args(["-fsS", "--max-time", TRANSFER_TIMEOUT_SECS, "-o", "/dev/null", "-X", "PUT", "-H"])
        .arg(format!("Content-Type: {mime}"))
        .arg("--data-binary")
        .arg(format!("@{}", file.display()))
        .args(["--", url])
        .status()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if status.success() { Ok(()) } else { Err(curl_failed("upload", status)) }
}

/// Downloads what Claude reported for one scene into `dir`. The file is kept even when the text
/// that was spoken or drawn differs from what was asked for, since the credits are spent.
pub fn save_outcome(
    kind: &str,
    position: i64,
    expected_text: &str,
    estimate_credits: Option<f64>,
    outcome: Option<&Outcome>,
    dir: &Path,
) -> Result<Saved, String> {
    let outcome = outcome.ok_or("Claude returned no result for this scene")?;
    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }
    let url = outcome.url.as_deref().ok_or("the result has no download URL")?;
    let path = next_versioned_path(dir, position, kind, &file_extension(url, kind));
    download(url, &path)?;
    if outcome.prompt.as_deref().is_some_and(|used| used.trim() != expected_text.trim()) {
        return Err(format!("the text was altered on the way; the file was kept at {}", path.display()));
    }
    Ok(Saved { path: path.display().to_string(), credits: outcome.credits.or(estimate_credits).unwrap_or(0.0) })
}

const NOT_STARTED: &str =
    "story_id = ?1 AND position = ?2 AND kind = ?3 AND status = 'requested' AND started_at IS NULL";
const REQUESTED: &str = "story_id = ?1 AND position = ?2 AND kind = ?3 AND status = 'requested'";

pub fn save_estimates(conn: &Connection, story_id: i64, kind: &str, estimates: &[(i64, f64)]) -> Result<(), AppError> {
    for (position, credits) in estimates {
        conn.execute(
            &format!("UPDATE scene_assets SET estimate_credits = ?4 WHERE {NOT_STARTED}"),
            params![story_id, position, kind, credits],
        )?;
    }
    Ok(())
}

/// Shows «генерируется…» for these items in the app.
pub fn mark_started(conn: &Connection, story_id: i64, kind: &str, positions: &[i64]) -> Result<(), AppError> {
    for position in positions {
        conn.execute(
            &format!("UPDATE scene_assets SET started_at = CURRENT_TIMESTAMP WHERE {NOT_STARTED}"),
            params![story_id, position, kind],
        )?;
    }
    Ok(())
}

pub fn apply_results(conn: &Connection, story_id: i64, kind: &str, results: &[ItemResult]) -> Result<(), AppError> {
    for result in results {
        match &result.saved {
            Ok(saved) => conn.execute(
                &format!(
                    "UPDATE scene_assets SET status = 'ready', file_path = ?4, credits = ?5, error = NULL,
                     updated_at = CURRENT_TIMESTAMP WHERE {REQUESTED}"
                ),
                params![story_id, result.position, kind, saved.path, saved.credits],
            )?,
            Err(message) => conn.execute(
                &format!(
                    "UPDATE scene_assets SET status = 'failed', error = ?4, started_at = NULL,
                     updated_at = CURRENT_TIMESTAMP WHERE {REQUESTED}"
                ),
                params![story_id, result.position, kind, message],
            )?,
        };
    }
    Ok(())
}

/// The whole run failed (Claude could not be reached or answered nonsense): fail every started item.
pub fn fail_started(
    conn: &Connection,
    story_id: i64,
    kind: &str,
    positions: &[i64],
    message: &str,
) -> Result<(), AppError> {
    let results: Vec<ItemResult> =
        positions.iter().map(|&position| ItemResult { position, saved: Err(message.to_owned()) }).collect();
    apply_results(conn, story_id, kind, &results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, NewScene};
    use crate::media::{self, KIND_IMAGE};

    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        let scenes = [
            NewScene { text: "[softly] one".into(), image_prompt: "fox".into() },
            NewScene { text: "two".into(), image_prompt: "owl".into() },
        ];
        db::replace_scenes(&mut conn, story.id, &scenes).unwrap();
        (conn, story.id)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("audiobooks-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_estimates_even_when_wrapped_in_prose_or_fences() {
        let text = "Done:\n```json\n{\"estimates\":[{\"position\":1,\"credits\":400}],\"errors\":[]}\n```";
        assert_eq!(parse_estimates(text).unwrap(), vec![(1, 400.0)]);
    }

    #[test]
    fn estimate_without_prices_reports_the_reasons() {
        let text = r#"{"estimates":[],"errors":[{"position":0,"message":"not signed in"}]}"#;
        assert!(parse_estimates(text).unwrap_err().to_string().contains("not signed in"));
    }

    #[test]
    fn rejects_negative_prices_and_non_json() {
        assert!(parse_estimates(r#"{"estimates":[{"position":0,"credits":-1}]}"#).is_err());
        assert!(parse_estimates("} nonsense {").is_err());
    }

    #[test]
    fn parses_results_and_errors_per_position() {
        let text = r#"{"results":[{"position":0,"url":"https://x","credits":367,"prompt":"p"}],
                       "errors":[{"position":2,"message":"boom"}]}"#;
        let outcomes = parse_outcomes(text).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].credits, Some(367.0));
        assert_eq!(outcome_for(&outcomes, 2).unwrap().error.as_deref(), Some("boom"));
        assert!(outcome_for(&outcomes, 5).is_none());
    }

    #[test]
    fn only_elevenlabs_content_https_urls_are_trusted() {
        assert!(is_trusted_url("https://elevenlabsusercontent.com/a/b.mp3?x=1"));
        assert!(is_trusted_url("https://cdn.elevenlabsusercontent.com/a.mp3"));
        assert!(!is_trusted_url("http://elevenlabsusercontent.com/a.mp3"));
        assert!(!is_trusted_url("https://evilelevenlabsusercontent.com/a.mp3"));
        assert!(!is_trusted_url("https://elevenlabsusercontent.com@evil.com/a.mp3"));
        assert!(!is_trusted_url("https://evil.com/elevenlabsusercontent.com"));
        assert!(!is_trusted_url("https://elevenlabsusercontent.com/a b"));
    }

    #[test]
    fn next_versioned_path_never_reuses_a_name_across_extensions() {
        let dir = temp_dir("versioned");
        assert!(next_versioned_path(&dir, 0, "audio", "mp3").ends_with("scene-0-audio-v1.mp3"));
        std::fs::write(dir.join("scene-0-image-v1.webp"), b"x").unwrap();
        std::fs::write(dir.join("scene-0-image-v4.png"), b"x").unwrap();
        std::fs::write(dir.join("scene-1-image-v9.webp"), b"x").unwrap();
        assert!(next_versioned_path(&dir, 0, "image", "webp").ends_with("scene-0-image-v5.webp"));
    }

    #[test]
    fn image_extension_comes_from_the_url_with_a_safe_default() {
        assert_eq!(file_extension("https://h/a/content.PNG?sig=1", KIND_IMAGE), "png");
        assert_eq!(file_extension("https://h/a/content.exe", KIND_IMAGE), "webp");
        assert_eq!(file_extension("https://h/a/content", KIND_IMAGE), "webp");
        assert_eq!(file_extension("https://h/a/content.png", KIND_AUDIO), "mp3");
    }

    #[test]
    fn queued_rows_list_only_requested_not_started_items_of_the_kind() {
        let (conn, story) = setup();
        media::request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        media::request_asset(&conn, story, 1, KIND_AUDIO).unwrap();
        media::request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        mark_started(&conn, story, KIND_AUDIO, &[1]).unwrap();

        let rows = queued_rows(&conn, story, KIND_AUDIO).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].position, rows[0].source.as_str()), (0, "[softly] one"));
    }

    #[test]
    fn estimates_then_results_move_the_row_to_ready_or_failed() {
        let (conn, story) = setup();
        media::request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        media::request_asset(&conn, story, 1, KIND_IMAGE).unwrap();
        save_estimates(&conn, story, KIND_IMAGE, &[(0, 12.0), (1, 7.0)]).unwrap();
        mark_started(&conn, story, KIND_IMAGE, &[0, 1]).unwrap();
        let results = [
            ItemResult { position: 0, saved: Ok(Saved { path: "/m/scene-0-image-v1.webp".into(), credits: 12.0 }) },
            ItemResult { position: 1, saved: Err("boom".into()) },
        ];

        apply_results(&conn, story, KIND_IMAGE, &results).unwrap();

        let assets = media::list_assets(&conn, story).unwrap();
        assert_eq!(assets[0].status, "ready");
        assert_eq!(assets[0].file_path.as_deref(), Some("/m/scene-0-image-v1.webp"));
        assert_eq!((assets[1].status.as_str(), assets[1].error.as_deref()), ("failed", Some("boom")));
        assert!(!assets[1].in_progress);
    }

    #[test]
    fn results_of_one_kind_never_touch_the_other() {
        let (conn, story) = setup();
        media::request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        media::request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        mark_started(&conn, story, KIND_IMAGE, &[0]).unwrap();
        fail_started(&conn, story, KIND_IMAGE, &[0], "boom").unwrap();

        let assets = media::list_assets(&conn, story).unwrap();
        let audio = assets.iter().find(|a| a.kind == KIND_AUDIO).unwrap();
        assert_eq!(audio.status, "requested");
    }

    #[test]
    fn save_reports_a_missing_failed_or_untrusted_outcome_without_downloading() {
        let dir = temp_dir("save");
        assert!(save_outcome(KIND_AUDIO, 0, "t", None, None, &dir).is_err());
        let failed = Outcome { position: 0, url: None, credits: None, prompt: None, error: Some("nope".into()) };
        assert_eq!(save_outcome(KIND_AUDIO, 0, "t", None, Some(&failed), &dir).unwrap_err(), "nope");
        let evil = Outcome { url: Some("https://evil.com/a.mp3".into()), error: None, ..failed };
        assert!(save_outcome(KIND_AUDIO, 0, "t", None, Some(&evil), &dir).unwrap_err().contains("untrusted"));
    }

    #[test]
    fn uploads_to_untrusted_hosts_are_refused() {
        let err = upload("https://evil.com/put", Path::new("/etc/hosts"), "image/webp").unwrap_err();
        assert!(err.contains("untrusted"));
    }
}
