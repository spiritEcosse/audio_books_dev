mod actions;
mod chat;
mod characters;
mod claude;
mod db;
mod error;
mod audio_queue;
mod image_queue;
mod media;
mod media_queue;
mod profiles;
mod stats;
mod upscale;
mod video;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::{Emitter, Manager, State};

use db::{Message, Scene, Story, StorySettings};
use stats::StoryStats;
use error::AppError;

struct AppState {
    db: Mutex<Connection>,
    workspace: PathBuf,
    /// Set by `cancel_export`; the running export stops when it sees it.
    export_cancel: Arc<AtomicBool>,
}

impl AppState {
    fn profiles_dir(&self) -> PathBuf {
        self.workspace.join("profiles")
    }
}

impl AppState {
    fn conn(&self) -> Result<MutexGuard<'_, Connection>, AppError> {
        self.db.lock().map_err(|_| AppError::new("database lock poisoned"))
    }
}

const MODEL_SETTING: &str = "model";

#[derive(serde::Serialize)]
struct ModelsInfo {
    options: Vec<ModelOptionDto>,
    selected: String,
}

#[derive(serde::Serialize)]
struct ModelOptionDto {
    id: &'static str,
    label: &'static str,
}

fn selected_model(conn: &Connection) -> Result<String, AppError> {
    Ok(db::get_setting(conn, MODEL_SETTING)?
        .filter(|id| claude::is_known_model(id))
        .unwrap_or_else(|| claude::DEFAULT_MODEL.to_owned()))
}

const REVISION_PREVIEW_CHARS: usize = 120;

#[derive(serde::Serialize)]
struct RevisionDto {
    id: i64,
    kind: String,
    created_at: String,
    summary: String,
}

fn revision_summary(revision: &db::Revision) -> String {
    if revision.kind == actions::KIND_SCENES {
        let count = serde_json::from_str::<Vec<serde_json::Value>>(&revision.payload)
            .map(|scenes| scenes.len())
            .unwrap_or(0);
        return format!("{count} сцен");
    }
    revision.payload.chars().take(REVISION_PREVIEW_CHARS).collect()
}

#[derive(serde::Serialize)]
struct StoryDetail {
    story: Story,
    scenes: Vec<Scene>,
    messages: Vec<Message>,
    settings: StorySettings,
    stats: StoryStats,
}

fn load_detail(conn: &Connection, id: i64) -> Result<StoryDetail, AppError> {
    let story = db::get_story(conn, id)?;
    let scenes = db::list_scenes(conn, id)?;
    let settings = db::get_story_settings(conn, id, profiles::DEFAULT_PROFILE)?;
    Ok(StoryDetail {
        stats: stats::estimate(&story, &scenes, &settings),
        messages: db::list_messages(conn, id)?,
        story,
        scenes,
        settings,
    })
}

#[derive(serde::Serialize)]
struct ProfileDto {
    id: String,
    label: String,
    builtin: bool,
}

#[derive(serde::Serialize)]
struct ProfilesInfo {
    options: Vec<ProfileDto>,
    /// Folder where users can drop their own `.md` profiles.
    custom_dir: String,
}

#[tauri::command]
fn list_stories(state: State<'_, AppState>) -> Result<Vec<Story>, AppError> {
    db::list_stories(&*state.conn()?)
}

#[tauri::command]
fn create_story(state: State<'_, AppState>, title: String, text: String) -> Result<Story, AppError> {
    let (title, text) = (title.trim(), text.trim());
    if title.is_empty() || text.is_empty() {
        return Err(AppError::new("title and text must not be empty"));
    }
    db::create_story(&*state.conn()?, title, text)
}

#[tauri::command]
fn delete_story(state: State<'_, AppState>, id: i64) -> Result<(), AppError> {
    db::delete_story(&*state.conn()?, id)
}

#[tauri::command]
fn get_story(state: State<'_, AppState>, id: i64) -> Result<StoryDetail, AppError> {
    load_detail(&*state.conn()?, id)
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> ProfilesInfo {
    let dir = state.profiles_dir();
    ProfilesInfo {
        options: profiles::list(&dir)
            .into_iter()
            .map(|p| ProfileDto { id: p.id, label: p.label, builtin: p.builtin })
            .collect(),
        custom_dir: dir.display().to_string(),
    }
}

/// Saves audience, target length and writing profile. The Claude session is dropped so the
/// next message starts fresh under the new system prompt, rebuilt from the saved history.
#[tauri::command]
fn set_story_settings(
    state: State<'_, AppState>,
    id: i64,
    target_minutes: i64,
    age_group: String,
    profile: String,
) -> Result<StoryDetail, AppError> {
    if profiles::find(&state.profiles_dir(), &profile).is_none() {
        return Err(AppError::new(format!("unknown profile: {profile}")));
    }
    let conn = state.conn()?;
    db::get_story(&conn, id)?;
    let settings = StorySettings { target_minutes, age_group, profile };
    db::set_story_settings(&conn, id, &settings)?;
    db::delete_session(&conn, id)?;
    load_detail(&conn, id)
}

#[tauri::command]
fn get_models(state: State<'_, AppState>) -> Result<ModelsInfo, AppError> {
    Ok(ModelsInfo {
        options: claude::MODELS
            .iter()
            .map(|m| ModelOptionDto { id: m.id, label: m.label })
            .collect(),
        selected: selected_model(&*state.conn()?)?,
    })
}

#[tauri::command]
fn set_model(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    if !claude::is_known_model(&id) {
        return Err(AppError::new(format!("unknown model: {id}")));
    }
    db::set_setting(&*state.conn()?, MODEL_SETTING, &id)
}

#[tauri::command]
fn accept_proposal(state: State<'_, AppState>, message_id: i64) -> Result<StoryDetail, AppError> {
    let conn = state.conn()?;
    let story_id = actions::accept_proposal(&conn, message_id)?;
    load_detail(&conn, story_id)
}

#[tauri::command]
fn reject_proposal(state: State<'_, AppState>, message_id: i64) -> Result<StoryDetail, AppError> {
    let conn = state.conn()?;
    let story_id = actions::reject_proposal(&conn, message_id)?;
    load_detail(&conn, story_id)
}

#[tauri::command]
fn list_revisions(state: State<'_, AppState>, story_id: i64) -> Result<Vec<RevisionDto>, AppError> {
    Ok(db::list_revisions(&*state.conn()?, story_id)?
        .into_iter()
        .map(|r| RevisionDto {
            summary: revision_summary(&r),
            id: r.id,
            kind: r.kind,
            created_at: r.created_at,
        })
        .collect())
}

#[tauri::command]
fn restore_revision(state: State<'_, AppState>, revision_id: i64) -> Result<StoryDetail, AppError> {
    let mut conn = state.conn()?;
    let story_id = actions::restore_revision(&mut conn, revision_id)?;
    load_detail(&conn, story_id)
}

#[derive(serde::Serialize)]
struct MediaInfo {
    story_id: i64,
    image_model: String,
    image_models: &'static [media::ImageModel],
    voice_id: String,
    voices: &'static [media::Voice],
    speech_credits_per_char: f64,
    prices_measured_on: &'static str,
    media_dir: String,
    assets: Vec<media::SceneAsset>,
    references: Vec<media::SceneReference>,
    /// Recurring characters of the story and the scenes each one is switched off for.
    characters: Vec<characters::Character>,
    character_off: Vec<characters::SceneCharacterOff>,
    /// Finished videos (1080p / 4K) in the story's media folder.
    videos: Vec<video::VideoFile>,
}

fn load_media(state: &AppState, conn: &Connection, story_id: i64) -> Result<MediaInfo, AppError> {
    db::get_story(conn, story_id)?;
    Ok(MediaInfo {
        story_id,
        image_model: media::image_model(conn, story_id)?,
        image_models: &media::IMAGE_MODELS,
        voice_id: media::voice_id(conn, story_id)?,
        voices: &media::VOICES,
        speech_credits_per_char: media::SPEECH_CREDITS_PER_CHAR,
        prices_measured_on: media::PRICES_MEASURED_ON,
        media_dir: media::story_media_dir(&state.workspace, story_id).display().to_string(),
        assets: media::list_assets(conn, story_id)?,
        references: media::list_references(conn, story_id)?,
        characters: characters::list(conn, story_id)?,
        character_off: characters::list_off(conn, story_id)?,
        videos: video::existing_videos(&media::story_media_dir(&state.workspace, story_id)),
    })
}

#[tauri::command]
fn get_media(state: State<'_, AppState>, story_id: i64) -> Result<MediaInfo, AppError> {
    load_media(&state, &*state.conn()?, story_id)
}

#[tauri::command]
fn set_image_model(state: State<'_, AppState>, story_id: i64, model_id: String) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::set_image_model(&conn, story_id, &model_id)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn set_voice(state: State<'_, AppState>, story_id: i64, voice_id: String) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::set_voice(&conn, story_id, &voice_id)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn request_asset(
    state: State<'_, AppState>,
    story_id: i64,
    position: i64,
    kind: String,
) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::request_asset(&conn, story_id, position, &kind)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn cancel_request(
    state: State<'_, AppState>,
    story_id: i64,
    position: i64,
    kind: String,
) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::cancel_request(&conn, story_id, position, &kind)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn cancel_queue(state: State<'_, AppState>, story_id: i64) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::cancel_queue(&conn, story_id)?;
    load_media(&state, &conn, story_id)
}

/// Whether a scene position lies in the optional inclusive range (no bound = open end).
fn in_range(position: i64, from: Option<i64>, to: Option<i64>) -> bool {
    from.is_none_or(|f| position >= f) && to.is_none_or(|t| position <= t)
}

/// Measures the exact price of the queued narration through Claude's ElevenLabs connector.
/// Nothing is charged.
#[tauri::command]
async fn estimate_audio_queue(
    state: State<'_, AppState>,
    story_id: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<MediaInfo, AppError> {
    let items: Vec<audio_queue::QueuedAudio> = {
        let conn = state.conn()?;
        db::get_story(&conn, story_id)?;
        audio_queue::queued_audio(&conn, story_id)?
            .into_iter()
            .filter(|item| in_range(item.position, from, to) && item.estimate_credits.is_none())
            .collect()
    };
    if items.is_empty() {
        // Pricing a whole story with nothing to price is a no-op; asking about one scene is a mistake.
        if from.is_none() && to.is_none() {
            let conn = state.conn()?;
            return load_media(&state, &conn, story_id);
        }
        return Err(AppError::new("no unpriced narration is waiting for this scene"));
    }
    let workspace = state.workspace.clone();
    let estimates = tauri::async_runtime::spawn_blocking(move || audio_queue::estimate(&workspace, &items))
        .await
        .map_err(|e| AppError::new(format!("estimate task failed: {e}")))??;
    let conn = state.conn()?;
    media_queue::save_estimates(&conn, story_id, media::KIND_AUDIO, &estimates)?;
    load_media(&state, &conn, story_id)
}

/// Generates the queued narration that already has an exact price (one take each). Spends credits.
#[tauri::command]
async fn generate_audio_queue(
    state: State<'_, AppState>,
    story_id: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<MediaInfo, AppError> {
    let items: Vec<audio_queue::QueuedAudio> = {
        let conn = state.conn()?;
        db::get_story(&conn, story_id)?;
        audio_queue::queued_audio(&conn, story_id)?
            .into_iter()
            .filter(|item| in_range(item.position, from, to))
            .collect()
    };
    if items.is_empty() {
        return Err(AppError::new("no narration is queued for generation"));
    }
    let positions: Vec<i64> = items.iter().map(|item| item.position).collect();
    {
        let conn = state.conn()?;
        media_queue::mark_started(&conn, story_id, media::KIND_AUDIO, &positions)?;
    }

    let workspace = state.workspace.clone();
    let media_dir = media::story_media_dir(&workspace, story_id);
    let outcome = tauri::async_runtime::spawn_blocking(move || audio_queue::generate(&workspace, &media_dir, &items))
        .await
        .map_err(|e| AppError::new(format!("generation task failed: {e}")))
        .and_then(|inner| inner);

    let conn = state.conn()?;
    match outcome {
        Ok(results) => media_queue::apply_results(&conn, story_id, media::KIND_AUDIO, &results)?,
        Err(err) => {
            let message = format!("generation failed, credits may already have been spent: {err}");
            media_queue::fail_started(&conn, story_id, media::KIND_AUDIO, &positions, &message)?;
        }
    }
    load_media(&state, &conn, story_id)
}

/// Measures the exact price of the queued images through Claude's ElevenLabs connector. Nothing is charged.
#[tauri::command]
async fn estimate_image_queue(
    state: State<'_, AppState>,
    story_id: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<MediaInfo, AppError> {
    let items: Vec<image_queue::QueuedImage> = {
        let conn = state.conn()?;
        db::get_story(&conn, story_id)?;
        image_queue::queued_images(&conn, story_id)?
            .into_iter()
            .filter(|item| in_range(item.position, from, to) && item.estimate_credits.is_none())
            .collect()
    };
    if items.is_empty() {
        if from.is_none() && to.is_none() {
            let conn = state.conn()?;
            return load_media(&state, &conn, story_id);
        }
        return Err(AppError::new("no unpriced image is waiting for this scene"));
    }
    let workspace = state.workspace.clone();
    let estimates = tauri::async_runtime::spawn_blocking(move || image_queue::estimate(&workspace, &items))
        .await
        .map_err(|e| AppError::new(format!("estimate task failed: {e}")))??;
    let conn = state.conn()?;
    media_queue::save_estimates(&conn, story_id, media::KIND_IMAGE, &estimates)?;
    load_media(&state, &conn, story_id)
}

/// Generates the queued images that already have an exact price (one take each), with scene 0 and the
/// scene's custom reference wired in. Scene 0 goes first, so the other scenes use its new image as their
/// anchor; if it fails, the rest stay queued. Spends credits.
#[tauri::command]
async fn generate_image_queue(
    state: State<'_, AppState>,
    story_id: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<MediaInfo, AppError> {
    let mut generated_any = false;
    for anchor_batch in [true, false] {
        let items: Vec<image_queue::QueuedImage> = {
            let conn = state.conn()?;
            db::get_story(&conn, story_id)?;
            image_queue::queued_images(&conn, story_id)?
                .into_iter()
                .filter(|item| {
                    in_range(item.position, from, to)
                        && (item.position == image_queue::ANCHOR_POSITION) == anchor_batch
                })
                .collect()
        };
        if items.is_empty() {
            continue;
        }
        generated_any = true;
        let positions: Vec<i64> = items.iter().map(|item| item.position).collect();
        {
            let conn = state.conn()?;
            media_queue::mark_started(&conn, story_id, media::KIND_IMAGE, &positions)?;
        }

        let workspace = state.workspace.clone();
        let media_dir = media::story_media_dir(&workspace, story_id);
        let outcome =
            tauri::async_runtime::spawn_blocking(move || image_queue::generate(&workspace, &media_dir, &items))
                .await
                .map_err(|e| AppError::new(format!("generation task failed: {e}")))
                .and_then(|inner| inner);

        let conn = state.conn()?;
        let failed = match outcome {
            Ok(results) => {
                media_queue::apply_results(&conn, story_id, media::KIND_IMAGE, &results)?;
                results.iter().any(|r| r.saved.is_err())
            }
            Err(err) => {
                let message = format!("generation failed, credits may already have been spent: {err}");
                media_queue::fail_started(&conn, story_id, media::KIND_IMAGE, &positions, &message)?;
                true
            }
        };
        if anchor_batch && failed {
            break;
        }
    }
    if !generated_any {
        return Err(AppError::new("no images are queued for generation"));
    }
    let conn = state.conn()?;
    load_media(&state, &conn, story_id)
}

/// Where the bundled Upscayl may live: the app's resources, or the source tree when running in dev.
fn upscale_roots(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        roots.push(dir);
    }
    roots.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources")));
    roots
}

/// Finished images of a story (one scene, or all of them) that still lack the enlarged copy.
fn images_to_upscale(
    conn: &Connection,
    workspace: &std::path::Path,
    story_id: i64,
    position: Option<i64>,
    resolution: upscale::Resolution,
) -> Result<Vec<(i64, PathBuf)>, AppError> {
    let media_dir = media::story_media_dir(workspace, story_id);
    Ok(media::list_assets(conn, story_id)?
        .into_iter()
        .filter(|a| a.kind == media::KIND_IMAGE && a.status == media::STATUS_READY)
        .filter(|a| position.is_none_or(|p| p == a.position))
        .filter_map(|a| a.file_path.map(|path| (a.position, PathBuf::from(path))))
        .filter(|(_, path)| path.starts_with(&media_dir) && !upscale::upscaled_path(path, resolution).is_file())
        .collect())
}

/// Enlarges every target, carrying on past a failed one; reports all failures together. Blocking.
fn upscale_all(
    roots: &[PathBuf],
    targets: &[(i64, PathBuf)],
    resolution: upscale::Resolution,
) -> Result<(), AppError> {
    let tools = upscale::find_tools(roots)?;
    let failures: Vec<String> = targets
        .iter()
        .filter_map(|(position, path)| {
            upscale::upscale(path, resolution, &tools).err().map(|e| format!("scene {position}: {e}"))
        })
        .collect();
    if failures.is_empty() { Ok(()) } else { Err(AppError::new(failures.join("; "))) }
}

/// Upscayl + ffmpeg: writes `<image>-1080p.png` / `-4k.png` next to the finished image(s). Free, local, ~25 s per image.
#[tauri::command]
async fn upscale_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    story_id: i64,
    resolution: String,
    position: Option<i64>,
) -> Result<MediaInfo, AppError> {
    let resolution = upscale::Resolution::parse(&resolution)?;
    let targets = {
        let conn = state.conn()?;
        images_to_upscale(&conn, &state.workspace, story_id, position, resolution)?
    };
    if !targets.is_empty() {
        let roots = upscale_roots(&app);
        tauri::async_runtime::spawn_blocking(move || upscale_all(&roots, &targets, resolution))
            .await
            .map_err(|e| AppError::new(format!("upscaling stopped: {e}")))??;
    }
    let conn = state.conn()?;
    load_media(&state, &conn, story_id)
}

/// Scenes that have both a finished image and finished narration, in order; the error names what is missing.
fn export_sources(conn: &Connection, story_id: i64) -> Result<Vec<video::SceneSource>, AppError> {
    let assets = media::list_assets(conn, story_id)?;
    let ready_file = |position: i64, kind: &str| {
        assets
            .iter()
            .find(|a| a.position == position && a.kind == kind && a.status == media::STATUS_READY)
            .and_then(|a| a.file_path.as_ref())
            .map(PathBuf::from)
    };
    let mut sources = Vec::new();
    let mut missing = Vec::new();
    for scene in db::list_scenes(conn, story_id)? {
        match (ready_file(scene.position, media::KIND_IMAGE), ready_file(scene.position, media::KIND_AUDIO)) {
            (Some(image), Some(audio)) => sources.push(video::SceneSource { position: scene.position, image, audio }),
            _ => missing.push(scene.position.to_string()),
        }
    }
    if missing.is_empty() {
        Ok(sources)
    } else {
        Err(AppError::new(format!("scenes without a finished image or narration: {}", missing.join(", "))))
    }
}

const EXPORT_PROGRESS_EVENT: &str = "export-progress";

#[derive(Clone, serde::Serialize)]
struct ExportProgressEvent {
    story_id: i64,
    #[serde(flatten)]
    progress: video::Progress,
}

/// Joins every scene (zoom/pan clip + narration, 1 s fades) into `video-1080p.mp4` / `video-4k.mp4`. Long: minutes to
/// tens of minutes; clips of unchanged scenes are reused.
#[tauri::command]
async fn export_video(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    story_id: i64,
    resolution: String,
) -> Result<MediaInfo, AppError> {
    let resolution = upscale::Resolution::parse(&resolution)?;
    let sources = {
        let conn = state.conn()?;
        export_sources(&conn, story_id)?
    };
    let media_dir = media::story_media_dir(&state.workspace, story_id);
    state.export_cancel.store(false, Ordering::SeqCst);
    let cancel = Arc::clone(&state.export_cancel);
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut report = |progress| {
            let _ = app.emit(EXPORT_PROGRESS_EVENT, ExportProgressEvent { story_id, progress });
        };
        video::export(&sources, resolution, &media_dir, &cancel, &mut report)
    })
    .await
    .map_err(|e| AppError::new(format!("export stopped: {e}")))?;
    // A cancel is not an error: the finished clips stay cached and the panel just goes back to idle.
    match outcome {
        Ok(_) => {}
        Err(error) if video::was_cancelled(&error) => {}
        Err(error) => return Err(error),
    }
    let conn = state.conn()?;
    load_media(&state, &conn, story_id)
}

/// Stops the running video export (if any) within about a second.
#[tauri::command]
fn cancel_export(state: State<'_, AppState>) {
    state.export_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn request_missing_assets(state: State<'_, AppState>, story_id: i64) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    media::request_missing(&conn, story_id)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn set_scene_reference(
    state: State<'_, AppState>,
    story_id: i64,
    position: i64,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<MediaInfo, AppError> {
    let path = media::save_reference_file(&state.workspace, story_id, position, &file_name, &bytes)?;
    let new_path = path.display().to_string();
    let conn = state.conn()?;
    let previous = media::set_reference(&conn, story_id, position, &new_path)?;
    if let Some(previous) = previous.filter(|old| *old != new_path) {
        let _ = std::fs::remove_file(previous);
    }
    load_media(&state, &conn, story_id)
}

/// Adds a recurring character: its picture goes to the image model with every scene it is not switched off for.
#[tauri::command]
fn add_character(
    state: State<'_, AppState>,
    story_id: i64,
    name: String,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    characters::add(&conn, &state.workspace, story_id, &name, &file_name, &bytes)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn remove_character(state: State<'_, AppState>, story_id: i64, character_id: i64) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    if let Some(file) = characters::remove(&conn, story_id, character_id)? {
        let _ = std::fs::remove_file(file);
    }
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn set_scene_character(
    state: State<'_, AppState>,
    story_id: i64,
    position: i64,
    character_id: i64,
    included: bool,
) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    characters::set_in_scene(&conn, story_id, position, character_id, included)?;
    load_media(&state, &conn, story_id)
}

#[tauri::command]
fn clear_scene_reference(state: State<'_, AppState>, story_id: i64, position: i64) -> Result<MediaInfo, AppError> {
    let conn = state.conn()?;
    if let Some(previous) = media::clear_reference(&conn, story_id, position)? {
        let _ = std::fs::remove_file(previous);
    }
    load_media(&state, &conn, story_id)
}

/// One chat turn: saves the user message, asks Claude (resuming its session), applies any
/// scene changes and saves the answer. On failure the user message stays saved.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    id: i64,
    text: String,
) -> Result<StoryDetail, AppError> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(AppError::new("message must not be empty"));
    }

    let (story, scenes, history, session_id, model, notes, settings) = {
        let conn = state.conn()?;
        let story = db::get_story(&conn, id)?;
        let scenes = db::list_scenes(&conn, id)?;
        let history = db::list_messages(&conn, id)?;
        let session_id = db::get_session(&conn, id)?;
        let model = selected_model(&conn)?;
        let notes = db::list_notes(&conn, id)?;
        let settings = db::get_story_settings(&conn, id, profiles::DEFAULT_PROFILE)?;
        db::add_message(&conn, id, "user", &text, false)?;
        (story, scenes, history, session_id, model, notes, settings)
    };

    let note_ids: Vec<i64> = notes.iter().map(|n| n.id).collect();
    let note_texts: Vec<String> = notes.into_iter().map(|n| n.text).collect();
    let prompt_text = chat::with_notes(&note_texts, &text);

    let workspace = state.workspace.clone();
    let profile = profiles::resolve(&state.profiles_dir(), &settings.profile);
    let turn = tauri::async_runtime::spawn_blocking(move || {
        chat::run_turn(&chat::TurnInput {
            cwd: &workspace,
            story: &story,
            scenes: &scenes,
            settings: &settings,
            profile: &profile,
            history: &history,
            session_id: session_id.as_deref(),
            model: &model,
            user_text: &prompt_text,
        })
    })
    .await
    .map_err(|e| AppError::new(format!("chat task failed: {e}")))??;

    let mut conn = state.conn()?;
    if let Some(session_id) = &turn.session_id {
        db::set_session(&conn, id, session_id)?;
    }
    if let Some(new_scenes) = &turn.scenes {
        actions::apply_scenes(&mut conn, id, new_scenes)?;
    }
    let reply = db::add_message(&conn, id, "assistant", &turn.reply, turn.scenes.is_some())?;
    if let Some(story_text) = &turn.story_text {
        db::add_proposal(&conn, reply.id, story_text)?;
    }
    db::delete_notes(&conn, &note_ids)?;
    load_detail(&conn, id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            // Older builds wrote a CLAUDE.md here for the embedded console; it would now
            // contradict the chat protocol.
            let _ = std::fs::remove_file(data_dir.join("CLAUDE.md"));
            std::fs::create_dir_all(data_dir.join("profiles"))?;

            let conn = Connection::open(data_dir.join("audiobooks.db"))?;
            db::init(&conn)?;
            app.manage(AppState { db: Mutex::new(conn), workspace: data_dir, export_cancel: Arc::new(AtomicBool::new(false)) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stories,
            create_story,
            delete_story,
            get_story,
            get_models,
            list_profiles,
            set_story_settings,
            set_model,
            send_message,
            accept_proposal,
            reject_proposal,
            list_revisions,
            restore_revision,
            get_media,
            set_image_model,
            set_voice,
            request_asset,
            request_missing_assets,
            cancel_request,
            cancel_queue,
            estimate_audio_queue,
            generate_audio_queue,
            estimate_image_queue,
            generate_image_queue,
            set_scene_reference,
            clear_scene_reference,
            add_character,
            remove_character,
            set_scene_character,
            upscale_images,
            export_video,
            cancel_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
