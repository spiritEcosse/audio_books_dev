//! Per-scene generated media (images, narration) and the bridge to ElevenLabs Flows.
//!
//! The app cannot call Flows itself: Claude Code does, through its ElevenLabs connector. The app
//! records *requests* in `scene_assets`; Claude fulfils them, drops files into the story's media
//! folder and flips the row to `ready` (see "Media bridge" in CLAUDE.md).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::AppError;
use crate::{db, profiles};

pub const KIND_IMAGE: &str = "image";
pub const KIND_AUDIO: &str = "audio";
pub const KINDS: [&str; 2] = [KIND_IMAGE, KIND_AUDIO];

pub const STATUS_REQUESTED: &str = "requested";
pub const STATUS_READY: &str = "ready";

pub const AUDIO_MODEL: &str = "eleven_v4";
pub const DEFAULT_IMAGE_MODEL: &str = "bytedance-seedream-5-lite";
/// Narration price measured with `creative_generate_speech` (estimate_only): ~994 credits / 1000 chars.
pub const SPEECH_CREDITS_PER_CHAR: f64 = 1.0;
/// When the prices below were measured with `estimate_only`; refresh them when they drift.
pub const PRICES_MEASURED_ON: &str = "2026-11-25";

pub const FAIRY_TALE_PROFILE: &str = "fairy_tale";
/// Ana-Rita2: young British female voice, good for book narration.
pub const FAIRY_TALE_VOICE: &str = "wJqPPQ618aTW29mptyoc";
/// Hannibal: audiobook narrator, used for every profile other than fairy tales.
pub const DEFAULT_VOICE: &str = "HLXBCncM2sIxwTmiIZg8";

pub const MAX_REFERENCE_BYTES: usize = 10 * 1024 * 1024;
const REFERENCE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Serialize)]
pub struct ImageModel {
    pub id: &'static str,
    pub label: &'static str,
    /// Estimated credits for one image at default settings.
    pub credits: f64,
}

#[derive(Debug, Serialize)]
pub struct Voice {
    pub id: &'static str,
    pub label: &'static str,
    /// What the voice suits best (ElevenLabs' own description), shown under the picker.
    pub description: &'static str,
    /// Public sample the picker plays; no credits are spent.
    pub preview_url: &'static str,
}

/// English narrator voices offered in the picker (from `creative_list_voices`, 2026-11-25).
pub const VOICES: [Voice; 22] = [
    Voice { id: FAIRY_TALE_VOICE, label: "Ana-Rita2", description: "Young British Female Voice, Smooth, Expressive, Good for Book Narration", preview_url: "https://storage.googleapis.com/eleven-public-prod/JSdwiNguB8ejxX5k1f64Z6aLdP73/voices/wJqPPQ618aTW29mptyoc/7f5d3985-6999-49c4-8c53-e0f43ef5a334.mp3" },
    Voice { id: "dPKFsZN0BnPRUfVI2DUW", label: "Ana-Rita3", description: "Young Female; British Accent; Clear Diction; Soft Tone; Excellent for Story Narration", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/user/JSdwiNguB8ejxX5k1f64Z6aLdP73/voices/dPKFsZN0BnPRUfVI2DUW/3b0c53c1-4df8-4863-8cdb-493a50019946.mp3" },
    Voice { id: "JYUzXNeyCPmymMkfcicu", label: "Jon - Children's Storyteller", description: "Warm American male voice for children's audiobooks, kids' stories, bedtime stories, sleep stories, fairy tales, picture books, and read-alouds.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/7bdd69d9e581481a8ea5216493271f81/voices/JYUzXNeyCPmymMkfcicu/bOwbmMjhSIrRt7oyA8vd.mp3" },
    Voice { id: "wGcFBfKz5yUQqhqr0mVy", label: "Maria Moody | Grandmotherly Storykeeper", description: "As an octogenarian, her voice carries the grace of long living: patience in the pauses, wisdom in the restraint, and kindness that never feels rehearsed.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/0933a199cad04cb18ac7e32b9e8476a2/voices/wGcFBfKz5yUQqhqr0mVy/hmDeilAcXODFxnSV2olx.mp3" },
    Voice { id: "nTMUXLFSfbWmdKKy7nDC", label: "Grandpa Alder", description: "A warm, seasoned old-man voice with gentle rasp, slow-paced wisdom, and a comforting storytelling tone.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/66603e0f916e46318897455f0ada6a81/voices/nTMUXLFSfbWmdKKy7nDC/I0FAhCFwCQr54oxJLr1r.mp3" },
    Voice { id: "JBFqnCBsd6RMkjVDRZzb", label: "George - Warm, Captivating Storyteller", description: "Warm resonance that instantly captivates listeners.", preview_url: "https://api.us.elevenlabs.io/v1/voices/JBFqnCBsd6RMkjVDRZzb/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJwcmVtYWRlIiwiZmlsZW5hbWUiOiJlNjIwNmQxYS0wNzIxLTQ3ODctYWFmYi0wNmE2ZTcwNWNhYzUubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "G17SuINrv2H9FC6nvetn", label: "Christopher - Gentle and Trustworthy", description: "British male narrator, English, well-spoken, gentle and trustworthy voice. Great for audiobooks, podcasts and voiceovers.", preview_url: "https://storage.googleapis.com/eleven-public-prod/custom/voices/G17SuINrv2H9FC6nvetn/ofEmJDtVvrmeL3OEs0ig.mp3" },
    Voice { id: DEFAULT_VOICE, label: "Hannibal - British, Natural and Clear", description: "Professional Audiobook Narrator. British. Warm, sophisticated tones.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/acd46395ec5b45939dc6833c86bef36a/voices/HLXBCncM2sIxwTmiIZg8/0NrZQUouyWSX4N2IkKDO.mp3" },
    Voice { id: "VU16byTywsWv5JpI8rbc", label: "Ash - Calm & Magnetic Narrative", description: "A warm, composed male voice with an effortless sense of depth. Calm without sleepy... mysterious without being theatrical.", preview_url: "https://api.us.elevenlabs.io/v1/voices/VU16byTywsWv5JpI8rbc/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiIyNWY2MzVjMjk1MDY0YmRlYmQ5MzNlZWY4MDc5Y2EwNCIsImZpbGVuYW1lIjoieHA2NjlxY3E2V3UzTVRtSkFOOVoubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "SaqYcK3ZpDKBAImA8AdW", label: "JANE DOE", description: "Voice of a young, warm, smooth and friendly woman.Perfect for social media, audiobooks, stories.", preview_url: "https://api.us.elevenlabs.io/v1/voices/SaqYcK3ZpDKBAImA8AdW/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiIzZTM2MTk3MTE5ZTE0MGIzOWZiYmE2MGZkZDRjZTgzZCIsImZpbGVuYW1lIjoiYjdqRUFaZ2k1bmJkak94dlNGNU0ubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "g12yXXabMWeFoLMzxPrF", label: "Julie I", description: "A mature, warm, and soft voice with a calm, reflective, and intimate delivery. Compassionate and gentle, with a soothing presence.", preview_url: "https://api.us.elevenlabs.io/v1/voices/g12yXXabMWeFoLMzxPrF/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiJiMTcwMDI1ZDM2OTI0MTNmOWIyYjQ2ZGVjMjc1ZTc1NCIsImZpbGVuYW1lIjoiYmExYzc1ZWItNzJkNC00N2VkLThjNDUtZjEyMzM1ZWNhYTAxLm1wMyIsInRpbWVzdGFtcCI6MTc5NTYxODgwMDAwMDAwMH0%3D" },
    Voice { id: "kLhAstPcnnPxqzk6gS5i", label: "Rowan - Gentle, Soft-Spoken & Warm", description: "A calm, natural voice with soft-spoken delivery and clear, even pacing. Ideal for gentle, steady narration and audiobooks.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/e2c52f9641c54df8b869fd5d3c9ce4f4/voices/kLhAstPcnnPxqzk6gS5i/xJJZFNzM004Xa0NW2cRX.mp3" },
    Voice { id: "qSeXEcewz7tA0Q0qk9fH", label: "Victoria - Warm & Relatable Narrator", description: "Conversational and friendly VO for explainer videos, viral social media and modern brand ads. Warm, upbeat, and relatable.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/228d4047a7e64c3ca68131d246d93ef2/voices/qSeXEcewz7tA0Q0qk9fH/zDds5EkXR72GXolhj81V.mp3" },
    Voice { id: "AeRdCCKzvd23BpJoofzx", label: "Nathaniel - Engaging, British and Calm", description: "Nathaniel's voice has already captured the hearts of many on ElevenLabs. Building on that success, we decided to record him again, this time with a mysterious tone.", preview_url: "https://api.us.elevenlabs.io/v1/voices/AeRdCCKzvd23BpJoofzx/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiI1MTRkOTRlOTI0MWM0OGU4Yjc5MDUzNzU3MjljNDM2ZiIsImZpbGVuYW1lIjoiTFI5UURVdjd3VWRGQk41eGtZNUwubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "giAoKpl5weRTCJK7uB9b", label: "Owen – Engaging British Narrator", description: "Trained by the UK's leading broadcaster, this professional British male voice is perfect for engaging storytelling and long-form narration.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/f0453fa76e4e4a1e973d87b70665a591/voices/giAoKpl5weRTCJK7uB9b/Yv1QpsmTg5t6ccDghMdi.mp3" },
    Voice { id: "5cHw2FPV11tjurURIttW", label: "Navira K - Immersive Audiobook Narrator", description: "Captivating, Expressive, Smooth — Navira K reads fiction in English like she already knows the twist and can't wait for you to find it, a female tone that leans in on clues, softens for heartbreak…", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/514d94e9241c48e8b7905375729c436f/voices/5cHw2FPV11tjurURIttW/jyYO5KP9UbOMgdgLCACj.mp3" },
    Voice { id: "64BB2AQyI36004WgQGF3", label: "Rouell - Warm, Expressive, Storyteller", description: "A warm, expressive male baritone with a natural, measured cadence. Moves smoothly between calm, atmospheric storytelling, quiet mystery and suspense, raw emotional drama, and conversational dialogue.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/8a74f750454646828965079f89f9d3bf/voices/64BB2AQyI36004WgQGF3/IEfxKn7fEyY3dr0em61T.mp3" },
    Voice { id: "IzVcaH3klbTTKcAKHoLV", label: "Tammy - Expressive, Warm, Versatile", description: "A warm, mature female voice with a distinctive natural South African/British-influenced English accent. A naturally grounded tone and a slightly husky quality.", preview_url: "https://api.us.elevenlabs.io/v1/voices/IzVcaH3klbTTKcAKHoLV/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiI3YTNhODQ3NDFkMzE0NWNkOTRlODU1OTMyYWFjMjc0NiIsImZpbGVuYW1lIjoicGFRaGlqSzE2d2hDZEFRZFE4dk4ubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "cWp9JnUjm1q5tyh8NXZI", label: "Noah - Deep, Narration, Podcast, Calm", description: "Filipino - Deep, charming, narrative and manly. Can be used for vlogging, narration, even podcast.", preview_url: "https://api.us.elevenlabs.io/v1/voices/cWp9JnUjm1q5tyh8NXZI/previews/audio?payload=eyJ2b2ljZV9zb3VyY2UiOiJjdXN0b20iLCJ3b3Jrc3BhY2VfaWQiOiI1NjI2MzI2Mjc0ZWE0MjExYjRlOGRjNmE5NWRhMGI2MiIsImZpbGVuYW1lIjoiZ2s3Q0g1YnFjeVMxUEVsRE9Kd3UubXAzIiwidGltZXN0YW1wIjoxNzk1NjE4ODAwMDAwMDAwfQ%3D%3D" },
    Voice { id: "bfGb7JTLUnZebZRiFYyq", label: "Adam - Distinct, Deep and Engaging", description: "Distinct tone, deep pitch, engaging long-form voice-over and narration.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/b3709ae8839847b6a804736de5e5651c/voices/bfGb7JTLUnZebZRiFYyq/sQiMNu3kolXvAQGhCb2s.mp3" },
    Voice { id: "SAxJUlDKRc79XAyeWyMu", label: "Morgan - Deep, Powerful, and Confident", description: "This deep, resonant, and warm voice delivers your message with an unhurried, comforting wisdom that instantly builds trust and captivates your audience.", preview_url: "https://storage.googleapis.com/eleven-public-prod/database/workspace/ca3194d2100848df93e780e96d957ab0/voices/SAxJUlDKRc79XAyeWyMu/88f27994-c9e9-49db-9a4c-0dcd375e85da.mp3" },
    Voice { id: "hpp4J3VqNfWAUOO0d1Us", label: "Bella - Professional, Bright, Warm", description: "This voice is warm, bright, and professional, characterized by a Standard American accent and a polished, narrative quality.", preview_url: "https://storage.googleapis.com/eleven-public-prod/premade/voices/hpp4J3VqNfWAUOO0d1Us/dab0f5ba-3aa4-48a8-9fad-f138fea1126d.mp3" },
];

pub const IMAGE_MODELS: [ImageModel; 12] = [
    ImageModel { id: "bytedance-seedream-5.5", label: "Seedream 5.5", credits: 98.2 },
    ImageModel { id: "krea-3", label: "Krea 3", credits: 150.9 },
    ImageModel { id: "gpt-image-2", label: "GPT Image 2", credits: 184.6 },
    ImageModel { id: "gpt-image-2.5-sunburst", label: "GPT Image 2.5 Sunburst", credits: 190.4 },
    ImageModel { id: "bytedance-seedream-5-lite", label: "Seedream 5 Lite", credits: 212.1 },
    ImageModel { id: "recraft-v4.1", label: "Recraft V4.1", credits: 242.4 },
    ImageModel { id: "gemini-2.5-flash-image", label: "Nano Banana", credits: 251.5 },
    ImageModel { id: "flux-2-pro", label: "FLUX.2 Pro", credits: 272.7 },
    ImageModel { id: "flux-3-image", label: "FLUX 3 Image", credits: 290.9 },
    ImageModel { id: "ideogram-4.6", label: "Ideogram 4.6", credits: 363.6 },
    ImageModel { id: "gemini-3.1-flash-image", label: "Nano Banana 2", credits: 406.0 },
    ImageModel { id: "gemini-3-pro-image", label: "Nano Banana Pro", credits: 1218.1 },
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SceneAsset {
    pub position: i64,
    pub kind: String,
    pub status: String,
    pub file_path: Option<String>,
    /// The scene text / image prompt changed after this asset was requested.
    pub stale: bool,
    pub model_id: String,
    /// Narration voice the audio was requested with; None for images.
    pub voice_id: Option<String>,
    pub credits: Option<f64>,
    /// Exact price Claude measured with estimate_only before generating; cleared on re-request.
    pub estimate_credits: Option<f64>,
    /// Claude has started generating this queued item (it sets started_at when it launches).
    pub in_progress: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SceneReference {
    pub position: i64,
    pub file_path: String,
}

pub fn is_known_image_model(id: &str) -> bool {
    IMAGE_MODELS.iter().any(|m| m.id == id)
}

pub fn is_known_voice(id: &str) -> bool {
    VOICES.iter().any(|v| v.id == id)
}

pub fn default_voice_for(profile: &str) -> &'static str {
    if profile == FAIRY_TALE_PROFILE { FAIRY_TALE_VOICE } else { DEFAULT_VOICE }
}

/// The story's narration voice: the saved choice, else the default for its writing profile.
pub fn voice_id(conn: &Connection, story_id: i64) -> Result<String, AppError> {
    let stored: Option<String> = conn
        .query_row("SELECT voice_id FROM story_media WHERE story_id = ?1", [story_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    if let Some(id) = stored.filter(|id| is_known_voice(id)) {
        return Ok(id);
    }
    let settings = db::get_story_settings(conn, story_id, profiles::DEFAULT_PROFILE)?;
    Ok(default_voice_for(&settings.profile).to_owned())
}

pub fn set_voice(conn: &Connection, story_id: i64, voice_id: &str) -> Result<(), AppError> {
    if !is_known_voice(voice_id) {
        return Err(AppError::new(format!("unknown voice: {voice_id}")));
    }
    db::get_story(conn, story_id)?;
    conn.execute(
        "INSERT INTO story_media (story_id, voice_id) VALUES (?1, ?2)
         ON CONFLICT(story_id) DO UPDATE SET voice_id = excluded.voice_id",
        params![story_id, voice_id],
    )?;
    Ok(())
}

pub fn story_media_dir(workspace: &Path, story_id: i64) -> PathBuf {
    workspace.join("media").join(story_id.to_string())
}

pub fn image_model(conn: &Connection, story_id: i64) -> Result<String, AppError> {
    let stored: Option<String> = conn
        .query_row("SELECT image_model FROM story_media WHERE story_id = ?1", [story_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    Ok(stored
        .filter(|id| is_known_image_model(id))
        .unwrap_or_else(|| DEFAULT_IMAGE_MODEL.to_owned()))
}

pub fn set_image_model(conn: &Connection, story_id: i64, model_id: &str) -> Result<(), AppError> {
    if !is_known_image_model(model_id) {
        return Err(AppError::new(format!("unknown image model: {model_id}")));
    }
    db::get_story(conn, story_id)?;
    conn.execute(
        "INSERT INTO story_media (story_id, image_model) VALUES (?1, ?2)
         ON CONFLICT(story_id) DO UPDATE SET image_model = excluded.image_model",
        params![story_id, model_id],
    )?;
    Ok(())
}

fn model_for(conn: &Connection, story_id: i64, kind: &str) -> Result<String, AppError> {
    if kind == KIND_IMAGE {
        image_model(conn, story_id)
    } else {
        Ok(AUDIO_MODEL.to_owned())
    }
}

fn scene_source(conn: &Connection, story_id: i64, position: i64, kind: &str) -> Result<String, AppError> {
    let scene = db::list_scenes(conn, story_id)?
        .into_iter()
        .find(|scene| scene.position == position)
        .ok_or_else(|| AppError::new(format!("scene {position} not found")))?;
    Ok(if kind == KIND_IMAGE { scene.image_prompt } else { scene.text })
}

/// Queues (or re-queues) generation of one scene asset. An existing file stays visible until
/// the new one replaces it.
pub fn request_asset(conn: &Connection, story_id: i64, position: i64, kind: &str) -> Result<(), AppError> {
    if !KINDS.contains(&kind) {
        return Err(AppError::new(format!("unknown asset kind: {kind}")));
    }
    let source = scene_source(conn, story_id, position, kind)?;
    let model = model_for(conn, story_id, kind)?;
    let voice = if kind == KIND_AUDIO { Some(voice_id(conn, story_id)?) } else { None };
    conn.execute(
        "INSERT INTO scene_assets (story_id, position, kind, status, source, model_id, voice_id)
         VALUES (?1, ?2, ?3, 'requested', ?4, ?5, ?6)
         ON CONFLICT(story_id, position, kind) DO UPDATE SET
             status = 'requested', source = excluded.source, model_id = excluded.model_id,
             voice_id = excluded.voice_id, estimate_credits = NULL, started_at = NULL, error = NULL, updated_at = CURRENT_TIMESTAMP",
        params![story_id, position, kind, source, model, voice],
    )?;
    Ok(())
}

/// Queues every asset that is neither ready-and-current nor already queued. Returns how many.
pub fn request_missing(conn: &Connection, story_id: i64) -> Result<usize, AppError> {
    let assets = list_assets(conn, story_id)?;
    let mut queued = 0;
    for scene in db::list_scenes(conn, story_id)? {
        for kind in KINDS {
            let covered = assets.iter().any(|asset| {
                asset.position == scene.position
                    && asset.kind == kind
                    && (asset.status == STATUS_REQUESTED || (asset.status == STATUS_READY && !asset.stale))
            });
            if !covered {
                request_asset(conn, story_id, scene.position, kind)?;
                queued += 1;
            }
        }
    }
    Ok(queued)
}

pub fn list_assets(conn: &Connection, story_id: i64) -> Result<Vec<SceneAsset>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT a.position, a.kind, a.status, a.file_path, a.model_id, a.credits, a.error, a.voice_id, a.estimate_credits,
                a.status = 'requested' AND a.started_at IS NOT NULL,
                COALESCE(a.source <> CASE a.kind WHEN 'image' THEN s.image_prompt ELSE s.text END, 1)
         FROM scene_assets a
         LEFT JOIN scenes s ON s.story_id = a.story_id AND s.position = a.position
         WHERE a.story_id = ?1 ORDER BY a.position, a.kind",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(SceneAsset {
            position: row.get(0)?,
            kind: row.get(1)?,
            status: row.get(2)?,
            file_path: row.get(3)?,
            model_id: row.get(4)?,
            credits: row.get(5)?,
            error: row.get(6)?,
            voice_id: row.get(7)?,
            estimate_credits: row.get(8)?,
            in_progress: row.get(9)?,
            stale: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Stores the custom reference of a scene; returns the previous file so the caller can delete it.
pub fn set_reference(
    conn: &Connection,
    story_id: i64,
    position: i64,
    file_path: &str,
) -> Result<Option<String>, AppError> {
    scene_source(conn, story_id, position, KIND_IMAGE)?;
    let previous = clear_reference(conn, story_id, position)?;
    conn.execute(
        "INSERT INTO scene_references (story_id, position, file_path) VALUES (?1, ?2, ?3)",
        params![story_id, position, file_path],
    )?;
    Ok(previous)
}

/// Removes the custom reference of a scene; returns its file so the caller can delete it.
pub fn clear_reference(conn: &Connection, story_id: i64, position: i64) -> Result<Option<String>, AppError> {
    let previous: Option<String> = conn
        .query_row(
            "SELECT file_path FROM scene_references WHERE story_id = ?1 AND position = ?2",
            params![story_id, position],
            |row| row.get(0),
        )
        .optional()?;
    conn.execute(
        "DELETE FROM scene_references WHERE story_id = ?1 AND position = ?2",
        params![story_id, position],
    )?;
    Ok(previous)
}

pub fn list_references(conn: &Connection, story_id: i64) -> Result<Vec<SceneReference>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT position, file_path FROM scene_references WHERE story_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(SceneReference { position: row.get(0)?, file_path: row.get(1)? })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Lower-cased extension of an uploaded reference, if it is an allowed image type.
pub fn reference_extension(file_name: &str) -> Result<String, AppError> {
    let extension = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if REFERENCE_EXTENSIONS.contains(&extension.as_str()) {
        Ok(extension)
    } else {
        Err(AppError::new("reference must be a png, jpg or webp image"))
    }
}

/// Writes an uploaded reference under the story's media folder and returns its path.
pub fn save_reference_file(
    workspace: &Path,
    story_id: i64,
    position: i64,
    file_name: &str,
    bytes: &[u8],
) -> Result<PathBuf, AppError> {
    let extension = reference_extension(file_name)?;
    if bytes.is_empty() || bytes.len() > MAX_REFERENCE_BYTES {
        return Err(AppError::new("reference must be between 1 byte and 10 MB"));
    }
    let dir = story_media_dir(workspace, story_id).join("refs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::new(format!("cannot create {}: {e}", dir.display())))?;
    let path = dir.join(format!("scene-{position}.{extension}"));
    std::fs::write(&path, bytes)
        .map_err(|e| AppError::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScene;

    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        let scenes = [
            NewScene { text: "one".into(), image_prompt: "fox".into() },
            NewScene { text: "two".into(), image_prompt: "owl".into() },
        ];
        db::replace_scenes(&mut conn, story.id, &scenes).unwrap();
        (conn, story.id)
    }

    #[test]
    fn request_asset_queues_with_current_source_and_default_model() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        let assets = list_assets(&conn, story).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].status, STATUS_REQUESTED);
        assert_eq!(assets[0].model_id, DEFAULT_IMAGE_MODEL);
        assert!(!assets[0].stale);
    }

    #[test]
    fn request_asset_rejects_unknown_kind_and_scene() {
        let (conn, story) = setup();
        assert!(request_asset(&conn, story, 0, "video").is_err());
        assert!(request_asset(&conn, story, 9, KIND_AUDIO).is_err());
    }

    #[test]
    fn asset_becomes_stale_when_scene_text_changes() {
        let (mut conn, story) = setup();
        request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        let edited = [NewScene { text: "changed".into(), image_prompt: "fox".into() }];
        db::replace_scenes(&mut conn, story, &edited).unwrap();
        assert!(list_assets(&conn, story).unwrap()[0].stale);
    }

    #[test]
    fn asset_of_a_removed_scene_is_stale() {
        let (mut conn, story) = setup();
        request_asset(&conn, story, 1, KIND_IMAGE).unwrap();
        let only_first = [NewScene { text: "one".into(), image_prompt: "fox".into() }];
        db::replace_scenes(&mut conn, story, &only_first).unwrap();
        assert!(list_assets(&conn, story).unwrap()[0].stale);
    }

    #[test]
    fn re_request_keeps_existing_file_until_replaced() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        conn.execute(
            "UPDATE scene_assets SET status = 'ready', file_path = '/x.png' WHERE story_id = ?1",
            [story],
        )
        .unwrap();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        let asset = &list_assets(&conn, story).unwrap()[0];
        assert_eq!(asset.status, STATUS_REQUESTED);
        assert_eq!(asset.file_path.as_deref(), Some("/x.png"));
    }

    #[test]
    fn re_request_clears_a_previous_estimate() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        conn.execute("UPDATE scene_assets SET estimate_credits = 321.5 WHERE story_id = ?1", [story]).unwrap();
        assert_eq!(list_assets(&conn, story).unwrap()[0].estimate_credits, Some(321.5));
        request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        assert_eq!(list_assets(&conn, story).unwrap()[0].estimate_credits, None);
    }

    #[test]
    fn started_item_is_in_progress_until_re_requested_or_ready() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        assert!(!list_assets(&conn, story).unwrap()[0].in_progress);
        conn.execute("UPDATE scene_assets SET started_at = CURRENT_TIMESTAMP WHERE story_id = ?1", [story]).unwrap();
        assert!(list_assets(&conn, story).unwrap()[0].in_progress);
        conn.execute("UPDATE scene_assets SET status = 'ready' WHERE story_id = ?1", [story]).unwrap();
        assert!(!list_assets(&conn, story).unwrap()[0].in_progress);
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        assert!(!list_assets(&conn, story).unwrap()[0].in_progress);
    }

    #[test]
    fn request_missing_skips_ready_and_queued_assets() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        assert_eq!(request_missing(&conn, story).unwrap(), 3);
        assert_eq!(request_missing(&conn, story).unwrap(), 0);
    }

    #[test]
    fn image_model_defaults_and_rejects_unknown_ids() {
        let (conn, story) = setup();
        assert_eq!(image_model(&conn, story).unwrap(), DEFAULT_IMAGE_MODEL);
        assert!(set_image_model(&conn, story, "nope").is_err());
        set_image_model(&conn, story, "flux-2-pro").unwrap();
        assert_eq!(image_model(&conn, story).unwrap(), "flux-2-pro");
    }

    #[test]
    fn reference_roundtrip_returns_previous_file() {
        let (conn, story) = setup();
        assert_eq!(set_reference(&conn, story, 0, "/a.png").unwrap(), None);
        assert_eq!(set_reference(&conn, story, 0, "/b.png").unwrap().as_deref(), Some("/a.png"));
        assert_eq!(list_references(&conn, story).unwrap().len(), 1);
        assert_eq!(clear_reference(&conn, story, 0).unwrap().as_deref(), Some("/b.png"));
        assert!(list_references(&conn, story).unwrap().is_empty());
    }

    #[test]
    fn voice_defaults_by_profile_and_can_be_overridden() {
        let (conn, story) = setup();
        let default_profile = db::get_story_settings(&conn, story, profiles::DEFAULT_PROFILE).unwrap();
        let fairy = db::StorySettings { profile: FAIRY_TALE_PROFILE.into(), ..default_profile.clone() };
        db::set_story_settings(&conn, story, &fairy).unwrap();
        assert_eq!(voice_id(&conn, story).unwrap(), FAIRY_TALE_VOICE);
        let novel = db::StorySettings { profile: "novel".into(), ..default_profile };
        db::set_story_settings(&conn, story, &novel).unwrap();
        assert_eq!(voice_id(&conn, story).unwrap(), DEFAULT_VOICE);
        set_voice(&conn, story, "dPKFsZN0BnPRUfVI2DUW").unwrap();
        assert_eq!(voice_id(&conn, story).unwrap(), "dPKFsZN0BnPRUfVI2DUW");
        assert!(set_voice(&conn, story, "nope").is_err());
    }

    #[test]
    fn voice_list_has_unique_ids_playable_previews_and_both_defaults() {
        let mut ids: Vec<&str> = VOICES.iter().map(|v| v.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), VOICES.len());
        assert!(VOICES.iter().all(|v| v.preview_url.starts_with("https://") && !v.description.is_empty()));
        assert!(is_known_voice(FAIRY_TALE_VOICE) && is_known_voice(DEFAULT_VOICE));
    }

    #[test]
    fn audio_request_records_the_voice_and_image_request_does_not() {
        let (conn, story) = setup();
        request_asset(&conn, story, 0, KIND_AUDIO).unwrap();
        request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
        let assets = list_assets(&conn, story).unwrap();
        let audio = assets.iter().find(|a| a.kind == KIND_AUDIO).unwrap();
        let image = assets.iter().find(|a| a.kind == KIND_IMAGE).unwrap();
        assert_eq!(audio.voice_id.as_deref(), Some(voice_id(&conn, story).unwrap().as_str()));
        assert_eq!(image.voice_id, None);
    }

    #[test]
    fn reference_extension_allows_only_images() {
        assert_eq!(reference_extension("Pic.PNG").unwrap(), "png");
        assert!(reference_extension("script.sh").is_err());
        assert!(reference_extension("noext").is_err());
    }

    #[test]
    fn save_reference_file_rejects_oversized_and_empty_payloads() {
        let dir = std::env::temp_dir();
        assert!(save_reference_file(&dir, 1, 0, "a.png", &[]).is_err());
        let big = vec![0u8; MAX_REFERENCE_BYTES + 1];
        assert!(save_reference_file(&dir, 1, 0, "a.png", &big).is_err());
    }
}
