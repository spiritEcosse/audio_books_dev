//! Image generation driven from the app: prices and generates the queued scene images through Claude's
//! ElevenLabs connector (headless `claude -p`). The references of a scene are uploaded and wired into its
//! generation: the pictures of the story's characters (those not switched off for the scene) or, when the
//! story has none, the scene 0 image as a style anchor; then the scene's own custom reference. Claude
//! only calls the ElevenLabs tools; the app sends the files itself (a headless run has no shell).

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::characters;
use crate::claude;
use crate::error::AppError;
use crate::media::{self, KIND_IMAGE};
use crate::media_queue::{self, CONNECTOR_TOOLS, ItemResult, MEDIA_MODEL};

const ESTIMATE_BASE_TURNS: u32 = 6;
const UPLOAD_BASE_TURNS: u32 = 6;
const TURNS_PER_ITEM: u32 = 3;
const MAX_REFERENCE_UPLOAD_BYTES: u64 = 20 * 1024 * 1024;
pub const ANCHOR_POSITION: i64 = 0;

const ESTIMATE_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. Call creative_create_flow once and remember its flow_id.
2. For EVERY item call creative_generate_image with: prompt = the item's "prompt" copied character for character (never change, translate, fix, shorten or remove anything), model_id = the item's "model_id", flow_id = the flow from step 1, generations_count 1, estimate_only true. Make these calls in parallel in one step. Never call creative_generate_image without estimate_only true.
3. Reply with ONLY one JSON object and nothing else: {"estimates":[{"position":<int>,"credits":<number from the tool's estimate.credits>}],"errors":[{"position":<int>,"message":"<reason>"}]}"#;

const UPLOAD_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. Call creative_create_flow once and remember its flow_id.
2. For EVERY file call creative_create_asset_upload with name, mime_type and file_size copied exactly from the file entry (never change them). Make these calls in parallel in one step. You cannot upload bytes yourself; do not try.
3. Reply with ONLY one JSON object and nothing else: {"flow_id":"<the flow_id>","uploads":[{"key":"<the file's key>","asset_id":"<asset_id>","upload_url":"<upload_url exactly as returned>"}],"errors":[{"key":"<key>","message":"<reason>"}]}"#;

const GENERATE_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. The task names a flow_id; use it for every call (nodes of different flows cannot be connected). If it says none, call creative_create_flow once and use that flow_id.
2. For EVERY entry of "assets" call creative_finalize_asset_upload with its asset_id and the flow_id, in parallel in one step. Remember the node_id returned for each asset's key.
3. For EVERY item call creative_generate_image with: prompt = the item's "prompt" copied character for character (never change, translate, fix, shorten or remove anything), model_id = the item's "model_id", flow_id, connect_from = the node_ids belonging to the keys in the item's "references" (leave connect_from out when the item has none), generations_count 1, and NO estimate_only. Make these calls in parallel in one step. Call it at most once per item: a repeated call is charged again, so never retry, even after an error. Remember which session_id belongs to which item.
4. Call creative_get_flow_run_status with the flow_id and all session_ids. If not all_completed and not has_failures, call it again (you cannot sleep, just call again) until one of them is true.
5. Reply with ONLY one JSON object and nothing else: {"results":[{"position":<int>,"url":<the generation's content_url>,"credits":<its price.credits>,"prompt":<the generation's prompt exactly as the status tool returned it>}],"errors":[{"position":<int>,"message":"<reason>"}]}"#;

#[derive(Debug, Clone, PartialEq)]
pub struct QueuedImage {
    pub position: i64,
    pub prompt: String,
    pub model_id: String,
    pub estimate_credits: Option<f64>,
    /// Files to wire in as references: the characters of the scene, else the scene 0 anchor (for other scenes),
    /// and the scene's own custom one.
    pub references: Vec<PathBuf>,
}

/// The scene 0 image to use as the style reference. A scene 0 that is queued again still has its previous file
/// (it stays until the new one replaces it), and that file is the style the user approved, so the status does not matter.
fn anchor_image(conn: &Connection, story_id: i64) -> Result<Option<PathBuf>, AppError> {
    Ok(media::list_assets(conn, story_id)?
        .into_iter()
        .find(|a| a.position == ANCHOR_POSITION && a.kind == KIND_IMAGE)
        .and_then(|a| a.file_path)
        .map(PathBuf::from))
}

/// Queued images that generation has not started for yet, in scene order, with their references.
pub fn queued_images(conn: &Connection, story_id: i64) -> Result<Vec<QueuedImage>, AppError> {
    // With characters, their pictures carry the look of the story; the scene 0 anchor would only pull every scene
    // towards the composition and style of that one picture.
    let anchor = if characters::list(conn, story_id)?.is_empty() { anchor_image(conn, story_id)? } else { None };
    let custom = media::list_references(conn, story_id)?;
    media_queue::queued_rows(conn, story_id, KIND_IMAGE)?
        .into_iter()
        .map(|row| {
            let mut references: Vec<PathBuf> = Vec::new();
            if row.position != ANCHOR_POSITION {
                references.extend(anchor.clone());
            }
            references.extend(characters::files_for_scene(conn, story_id, row.position)?);
            references.extend(custom.iter().filter(|r| r.position == row.position).map(|r| PathBuf::from(&r.file_path)));
            Ok(QueuedImage {
                position: row.position,
                prompt: row.source,
                model_id: row.model_id,
                estimate_credits: row.estimate_credits,
                references,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct ReferenceFile {
    key: String,
    path: PathBuf,
    mime: &'static str,
    size: u64,
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The distinct, uploadable reference files of the items (existing, known type, sane size).
fn reference_files(items: &[QueuedImage]) -> Vec<ReferenceFile> {
    let mut files: Vec<ReferenceFile> = Vec::new();
    for path in items.iter().flat_map(|item| item.references.iter()) {
        if files.iter().any(|f| &f.path == path) {
            continue;
        }
        let Some(mime) = mime_for(path) else { continue };
        let Ok(meta) = std::fs::metadata(path) else { continue };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_REFERENCE_UPLOAD_BYTES {
            continue;
        }
        files.push(ReferenceFile { key: files.len().to_string(), path: path.clone(), mime, size: meta.len() });
    }
    files
}

fn keys_of(item: &QueuedImage, files: &[ReferenceFile]) -> Vec<String> {
    files.iter().filter(|f| item.references.contains(&f.path)).map(|f| f.key.clone()).collect()
}

fn estimate_json(items: &[QueuedImage]) -> Vec<Value> {
    items.iter().map(|i| json!({ "position": i.position, "prompt": i.prompt, "model_id": i.model_id })).collect()
}

/// Exact prices (estimate_only, nothing is charged) for the queued images. References do not change the
/// price, so none are uploaded. Blocking.
pub fn estimate(cwd: &Path, items: &[QueuedImage]) -> Result<Vec<(i64, f64)>, AppError> {
    let prompt = media_queue::items_prompt("Estimate the price of these images", estimate_json(items));
    let turns = ESTIMATE_BASE_TURNS + TURNS_PER_ITEM * items.len() as u32;
    let reply = claude::run_connector(cwd, MEDIA_MODEL, ESTIMATE_SYSTEM, &prompt, CONNECTOR_TOOLS, turns)?;
    let estimates = media_queue::parse_estimates(&reply.text)?;
    Ok(estimates.into_iter().filter(|(position, _)| items.iter().any(|i| i.position == *position)).collect())
}

/// Phase 1: Claude opens a flow and upload slots, the app PUTs the files. Returns the flow and (key, asset_id).
fn upload_references(cwd: &Path, files: &[ReferenceFile]) -> Result<(String, Vec<(String, String)>), AppError> {
    let list: Vec<Value> = files
        .iter()
        .map(|f| json!({ "key": f.key, "name": format!("reference-{}", f.key), "mime_type": f.mime, "file_size": f.size }))
        .collect();
    let prompt = media_queue::items_prompt("Create upload slots for these files", list);
    let turns = UPLOAD_BASE_TURNS + TURNS_PER_ITEM * files.len() as u32;
    let reply = claude::run_connector(cwd, MEDIA_MODEL, UPLOAD_SYSTEM, &prompt, CONNECTOR_TOOLS, turns)?;
    let json = media_queue::extract_json(&reply.text)?;
    let flow_id = json["flow_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AppError::new("Claude returned no flow for the references"))?
        .to_owned();
    let mut assets = Vec::new();
    for file in files {
        let slot = json["uploads"].as_array().into_iter().flatten().find(|u| u["key"].as_str() == Some(file.key.as_str()));
        let asset_id = slot.and_then(|u| u["asset_id"].as_str());
        let url = slot.and_then(|u| u["upload_url"].as_str());
        let (Some(asset_id), Some(url)) = (asset_id, url) else {
            return Err(AppError::new(format!("no upload slot was created for reference {}", file.key)));
        };
        media_queue::upload(url, &file.path, file.mime)
            .map_err(|e| AppError::new(format!("reference upload failed: {e}")))?;
        assets.push((file.key.clone(), asset_id.to_owned()));
    }
    Ok((flow_id, assets))
}

/// Generates the images (one take each) with their references wired in, downloads them into `dir` and
/// reports per scene. Spends credits. Blocking.
pub fn generate(cwd: &Path, dir: &Path, items: &[QueuedImage]) -> Result<Vec<ItemResult>, AppError> {
    let files = reference_files(items);
    let (flow_id, assets) = if files.is_empty() {
        (None, Vec::new())
    } else {
        let (flow_id, assets) = upload_references(cwd, &files)?;
        (Some(flow_id), assets)
    };
    let assets_json: Vec<Value> = assets.iter().map(|(key, id)| json!({ "key": key, "asset_id": id })).collect();
    let items_json: Vec<Value> = items
        .iter()
        .map(|i| json!({ "position": i.position, "prompt": i.prompt, "model_id": i.model_id, "references": keys_of(i, &files) }))
        .collect();
    let prompt = format!(
        "Generate these images.\nflow_id: {}\nassets: {}\nitems: {}",
        flow_id.as_deref().unwrap_or("none"),
        Value::Array(assets_json),
        Value::Array(items_json)
    );
    let turns = media_queue::GENERATE_BASE_TURNS + TURNS_PER_ITEM * (items.len() + assets.len()) as u32;
    let reply = claude::run_connector(cwd, MEDIA_MODEL, GENERATE_SYSTEM, &prompt, CONNECTOR_TOOLS, turns)?;
    let outcomes = media_queue::parse_outcomes(&reply.text)?;
    Ok(items
        .iter()
        .map(|item| ItemResult {
            position: item.position,
            saved: media_queue::save_outcome(
                KIND_IMAGE,
                item.position,
                &item.prompt,
                item.estimate_credits,
                media_queue::outcome_for(&outcomes, item.position),
                dir,
            ),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, NewScene};

    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        let scenes = ["a", "b", "c"]
            .map(|text| NewScene { text: text.into(), image_prompt: format!("prompt {text}") });
        db::replace_scenes(&mut conn, story.id, &scenes).unwrap();
        (conn, story.id)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("audiobooks-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(position: i64, references: Vec<PathBuf>) -> QueuedImage {
        QueuedImage { position, prompt: "p".into(), model_id: "m".into(), estimate_credits: None, references }
    }

    #[test]
    fn other_scenes_get_the_ready_anchor_and_their_own_reference() {
        let (conn, story) = setup();
        for position in 0..3 {
            media::request_asset(&conn, story, position, KIND_IMAGE).unwrap();
        }
        conn.execute(
            "UPDATE scene_assets SET status = 'ready', file_path = '/m/scene-0-image-v1.webp' WHERE position = 0",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO scene_references (story_id, position, file_path) VALUES (?1, 2, '/r/owl.png')", [story])
            .unwrap();

        let items = queued_images(&conn, story).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].references, [PathBuf::from("/m/scene-0-image-v1.webp")]);
        assert_eq!(
            items[1].references,
            [PathBuf::from("/m/scene-0-image-v1.webp"), PathBuf::from("/r/owl.png")]
        );
    }

    #[test]
    fn a_requeued_scene_zero_still_serves_as_the_anchor_with_its_old_file() {
        let (conn, story) = setup();
        for position in 0..2 {
            media::request_asset(&conn, story, position, KIND_IMAGE).unwrap();
        }
        conn.execute("UPDATE scene_assets SET file_path = '/m/scene-0-image-v1.webp' WHERE position = 0", []).unwrap();

        let items = queued_images(&conn, story).unwrap();

        assert_eq!(items.len(), 2, "both scenes are still queued");
        assert!(items[0].references.is_empty(), "scene 0 never references itself");
        assert_eq!(items[1].references, [PathBuf::from("/m/scene-0-image-v1.webp")]);
    }

    #[test]
    fn without_any_scene_zero_file_other_scenes_get_no_anchor() {
        let (conn, story) = setup();
        for position in 0..2 {
            media::request_asset(&conn, story, position, KIND_IMAGE).unwrap();
        }
        let items = queued_images(&conn, story).unwrap();
        assert!(items[1].references.is_empty());
    }

    fn add_character_row(conn: &Connection, story: i64, name: &str, path: &str) -> i64 {
        conn.execute("INSERT INTO story_characters (story_id, name, file_path) VALUES (?1, ?2, ?3)", rusqlite::params![story, name, path])
            .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn characters_replace_the_anchor_and_come_before_the_scenes_own_reference() {
        let (conn, story) = setup();
        for position in 0..3 {
            media::request_asset(&conn, story, position, KIND_IMAGE).unwrap();
        }
        conn.execute("UPDATE scene_assets SET file_path = '/m/scene-0-image-v1.webp' WHERE position = 0", []).unwrap();
        conn.execute("INSERT INTO scene_references (story_id, position, file_path) VALUES (?1, 2, '/r/owl.png')", [story]).unwrap();
        add_character_row(&conn, story, "Tembo", "/c/tembo.png");

        let items = queued_images(&conn, story).unwrap();

        assert_eq!(items[0].references, [PathBuf::from("/c/tembo.png")], "scene 0 gets the hero too unless it is switched off");
        assert_eq!(items[1].references, [PathBuf::from("/c/tembo.png")], "no scene 0 anchor once there are characters");
        assert_eq!(items[2].references, [PathBuf::from("/c/tembo.png"), PathBuf::from("/r/owl.png")]);
    }

    #[test]
    fn a_character_switched_off_for_a_scene_is_not_sent_for_it() {
        let (conn, story) = setup();
        for position in 0..2 {
            media::request_asset(&conn, story, position, KIND_IMAGE).unwrap();
        }
        let hero = add_character_row(&conn, story, "Tembo", "/c/tembo.png");
        add_character_row(&conn, story, "Owl", "/c/owl.png");
        characters::set_in_scene(&conn, story, 0, hero, false).unwrap();

        let items = queued_images(&conn, story).unwrap();

        assert_eq!(items[0].references, [PathBuf::from("/c/owl.png")]);
        assert_eq!(items[1].references, [PathBuf::from("/c/tembo.png"), PathBuf::from("/c/owl.png")]);
    }

    #[test]
    fn scene_zero_never_references_itself() {
        let (conn, story) = setup();
        media::request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        let items = queued_images(&conn, story).unwrap();
        assert!(items[0].references.is_empty());
    }

    #[test]
    fn reference_files_are_distinct_existing_and_of_a_known_type() {
        let dir = temp_dir("refs");
        let (png, webp, text) = (dir.join("a.png"), dir.join("b.webp"), dir.join("c.txt"));
        for path in [&png, &webp, &text] {
            std::fs::write(path, b"x").unwrap();
        }
        let items = [
            item(1, vec![png.clone(), webp.clone(), text.clone(), dir.join("missing.png")]),
            item(2, vec![png.clone()]),
        ];

        let files = reference_files(&items);

        assert_eq!(files.iter().map(|f| f.mime).collect::<Vec<_>>(), ["image/png", "image/webp"]);
        assert_eq!(keys_of(&items[0], &files), ["0", "1"]);
        assert_eq!(keys_of(&items[1], &files), ["0"]);
    }

    #[test]
    fn a_single_image_with_a_reference_gets_a_long_polling_budget() {
        // finalize the reference + generate + status polls while a ~1 min generation runs
        let turns = media_queue::GENERATE_BASE_TURNS + TURNS_PER_ITEM * 2;
        assert!(turns >= 40, "{turns} turns ended a real run with the image generated but not downloaded");
    }

    #[test]
    fn prompts_use_the_image_tools() {
        assert!(claude::is_known_model(MEDIA_MODEL));
        for prompt in [ESTIMATE_SYSTEM, GENERATE_SYSTEM] {
            assert!(prompt.contains("creative_generate_image"));
        }
        assert!(UPLOAD_SYSTEM.contains("creative_create_asset_upload"));
        assert!(GENERATE_SYSTEM.contains("creative_finalize_asset_upload"));
    }
}
