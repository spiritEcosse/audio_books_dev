use serde_json::{Value, json};

use crate::db::NewScene;
use crate::error::AppError;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-sonnet-5-5";
const MAX_TOKENS: u32 = 8192;

const SYSTEM_PROMPT: &str = "You split a story into scenes for an illustrated audiobook video. \
Keep the original text intact and in order: each scene's \"text\" is a consecutive passage \
meant to be narrated aloud (roughly 20-60 seconds). \"image_prompt\" is a vivid English \
description of one still image for that scene. Reply with ONLY a JSON array of objects \
with keys \"text\" and \"image_prompt\", no commentary.";

pub async fn split_into_scenes(api_key: &str, story_text: &str) -> Result<Vec<NewScene>, AppError> {
    let body = json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": [{ "role": "user", "content": story_text }],
    });

    let response = reqwest::Client::new()
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    let payload: Value = response.json().await?;
    if !status.is_success() {
        let message = payload["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(AppError::new(format!("Claude API {status}: {message}")));
    }

    let text = payload["content"][0]["text"]
        .as_str()
        .ok_or_else(|| AppError::new("Claude returned no text content"))?;
    parse_scenes(text)
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
