//! Narration generation driven from the app: prices and generates the queued audio of a story through
//! Claude's ElevenLabs connector (headless `claude -p`). Claude only calls the ElevenLabs tools and
//! reports back JSON; files and database bookkeeping live in `media_queue`.

use std::path::Path;

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::claude;
use crate::collect::{self, StartReport};
use crate::error::AppError;
use crate::media::{self, KIND_AUDIO};
use crate::media_queue::{self, CONNECTOR_TOOLS, MEDIA_MODEL};

const ESTIMATE_BASE_TURNS: u32 = 6;
const TURNS_PER_ITEM: u32 = 2;

/// The prompts name the narration model literally; `narration_model_matches_the_prompts` guards it.
const ESTIMATE_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. Call creative_create_flow once and remember its flow_id.
2. For EVERY item call creative_generate_speech with: prompt = the item's "text" copied character for character (never change, translate, fix, shorten or remove anything, including [square bracket tags]), model_id "eleven_v4", voice_id = the item's "voice_id", flow_id = the flow from step 1, generations_count 1, estimate_only true. Make these calls in parallel in one step. Never call creative_generate_speech without estimate_only true.
3. Reply with ONLY one JSON object and nothing else: {"estimates":[{"position":<int>,"credits":<number from the tool's estimate.credits>}],"errors":[{"position":<int>,"message":"<reason>"}]}"#;

const GENERATE_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. Call creative_create_flow once and remember its flow_id.
2. For EVERY item call creative_generate_speech with: prompt = the item's "text" copied character for character (never change, translate, fix, shorten or remove anything, including [square bracket tags]), model_id "eleven_v4", voice_id = the item's "voice_id", flow_id = the flow from step 1, generations_count 1, and NO estimate_only. Make these calls in parallel in one step. Call it at most once per item: a repeated call is charged again, so never retry, even after an error. Remember which session_id belongs to which item.
3. Do NOT wait for the generations and do NOT call creative_get_flow_run_status: the app collects the results later. Reply right away with ONLY one JSON object and nothing else: {"started":[{"position":<int>,"flow_id":"<the flow_id>","session_id":"<the session_id returned for that item's generate call>"}],"errors":[{"position":<int>,"message":"<reason>"}]}"#;

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedAudio {
    pub position: i64,
    pub text: String,
    pub voice_id: String,
    pub estimate_credits: Option<f64>,
}

/// Queued narration that generation has not started for yet, in scene order.
pub fn queued_audio(conn: &Connection, story_id: i64) -> Result<Vec<QueuedAudio>, AppError> {
    let fallback_voice = media::voice_id(conn, story_id)?;
    Ok(media_queue::queued_rows(conn, story_id, KIND_AUDIO)?
        .into_iter()
        .map(|row| QueuedAudio {
            position: row.position,
            text: row.source,
            voice_id: row.voice_id.unwrap_or_else(|| fallback_voice.clone()),
            estimate_credits: row.estimate_credits,
        })
        .collect())
}

fn items_json(items: &[QueuedAudio]) -> Vec<Value> {
    items.iter().map(|i| json!({ "position": i.position, "text": i.text, "voice_id": i.voice_id })).collect()
}

/// Exact prices (estimate_only, nothing is charged) for the queued narration. Blocking.
pub fn estimate(cwd: &Path, items: &[QueuedAudio]) -> Result<Vec<(i64, f64)>, AppError> {
    let prompt = media_queue::items_prompt("Estimate the price of these narration items", items_json(items));
    let turns = ESTIMATE_BASE_TURNS + TURNS_PER_ITEM * items.len() as u32;
    let reply = claude::run_connector(cwd, MEDIA_MODEL, ESTIMATE_SYSTEM, &prompt, CONNECTOR_TOOLS, turns)?;
    let estimates = media_queue::parse_estimates(&reply.text)?;
    Ok(estimates.into_iter().filter(|(position, _)| items.iter().any(|i| i.position == *position)).collect())
}

/// Launches the generations (one take each). Spends credits. Answers as soon as every one is started, with the ids
/// to collect the results by. Blocking.
pub fn start(cwd: &Path, items: &[QueuedAudio]) -> Result<StartReport, AppError> {
    let prompt = media_queue::items_prompt("Generate narration for these items", items_json(items));
    let turns = collect::START_BASE_TURNS + TURNS_PER_ITEM * items.len() as u32;
    let reply = claude::run_connector(cwd, MEDIA_MODEL, GENERATE_SYSTEM, &prompt, CONNECTOR_TOOLS, turns)?;
    collect::parse_started(&reply.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, NewScene};

    #[test]
    fn queued_audio_carries_the_scene_text_and_the_story_voice() {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        db::replace_scenes(&mut conn, story.id, &[NewScene { text: "[softly] one".into(), image_prompt: "fox".into() }])
            .unwrap();
        media::request_asset(&conn, story.id, 0, KIND_AUDIO).unwrap();

        let items = queued_audio(&conn, story.id).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "[softly] one");
        assert_eq!(items[0].voice_id, media::voice_id(&conn, story.id).unwrap());
    }

    #[test]
    fn narration_model_matches_the_prompts() {
        assert!(claude::is_known_model(MEDIA_MODEL));
        assert!(ESTIMATE_SYSTEM.contains(media::AUDIO_MODEL) && GENERATE_SYSTEM.contains(media::AUDIO_MODEL));
    }
}
