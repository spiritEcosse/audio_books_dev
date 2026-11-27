//! Tauri commands for a story's upload.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use super::accounts::api;
use super::draft::{self, Draft};
use super::form::Form;
use super::job::{self, View};
use super::upload;
use crate::error::AppError;
use crate::{AppState, claude, db, media, selected_model, video};

const PROGRESS_EVENT: &str = "youtube-progress";

/// Work that runs one at a time and can be cancelled.
#[derive(Default)]
pub(super) struct Slot {
    running: Arc<AtomicBool>,
    pub(super) cancel: Arc<AtomicBool>,
}

/// Frees the slot when dropped.
pub(super) struct Taken(Arc<AtomicBool>);

impl Drop for Taken {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Slot {
    pub(super) fn take(&self, busy: &str) -> Result<(Taken, Arc<AtomicBool>), AppError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(busy));
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok((Taken(Arc::clone(&self.running)), Arc::clone(&self.cancel)))
    }
}

#[derive(Default)]
pub struct YoutubeState {
    pub(super) sign_in: Slot,
    pub(super) upload: Slot,
}

#[derive(Serialize)]
pub struct YoutubeInfo {
    uploading: bool,
    form: Form,
    video: Option<String>,
    uploads: Vec<ChannelUpload>,
}

#[derive(Serialize)]
pub struct ChannelUpload {
    channel_id: String,
    #[serde(flatten)]
    view: View,
}

#[tauri::command]
pub fn get_youtube(
    state: State<'_, AppState>,
    youtube: State<'_, YoutubeState>,
    story_id: i64,
) -> Result<YoutubeInfo, AppError> {
    let conn = state.conn()?;
    let form = match job::load_form(&conn, story_id)? {
        Some(form) => form,
        None => Form {
            title: db::get_story(&conn, story_id)?.title,
            ..Form::default()
        },
    };
    let uploads = job::load_records(&conn, story_id)?
        .into_iter()
        .map(|(channel_id, record)| ChannelUpload {
            channel_id,
            view: record.view(),
        })
        .collect();
    Ok(YoutubeInfo {
        uploading: youtube.upload.running.load(Ordering::SeqCst),
        form,
        video: video::existing_video(&media::story_media_dir(&state.workspace, story_id)),
        uploads,
    })
}

#[tauri::command]
pub fn save_youtube_form(
    state: State<'_, AppState>,
    story_id: i64,
    form: Form,
) -> Result<(), AppError> {
    job::save_form(&*state.conn()?, story_id, &form)
}

#[derive(Clone, Serialize)]
struct ProgressEvent {
    story_id: i64,
    percent: f64,
}

/// A title, description and tags written by Claude from the story; nothing is saved or uploaded.
#[tauri::command]
pub async fn draft_youtube(app: tauri::AppHandle, story_id: i64) -> Result<Draft, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (model, story) = {
            let conn = state.conn()?;
            let story = db::get_story(&conn, story_id)?;
            let scenes = db::list_scenes(&conn, story_id)?
                .into_iter()
                .map(|scene| scene.text)
                .collect::<Vec<_>>()
                .join("\n");
            let text = if story.text.trim().is_empty() {
                scenes
            } else {
                story.text
            };
            if text.trim().is_empty() {
                return Err(AppError::new("the story has no text yet"));
            }
            let prompt = format!("Working title: {}\n\n{text}", story.title);
            (selected_model(&conn)?, prompt)
        };
        let reply = claude::run(&state.workspace, &model, draft::SYSTEM_PROMPT, &story, None)?;
        draft::parse(&reply.text)
    })
    .await
    .map_err(|e| AppError::new(format!("the draft stopped: {e}")))?
}

fn unix_now() -> Result<i64, AppError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .map_err(|e| AppError::new(format!("the clock is wrong: {e}")))
}

fn upload_story(
    app: &tauri::AppHandle,
    story_id: i64,
    form: Form,
    again: bool,
    cancel: &AtomicBool,
) -> Result<Option<View>, AppError> {
    let state = app.state::<AppState>();
    let media_dir = media::story_media_dir(&state.workspace, story_id);
    let file = video::video_path(&media_dir);
    let metadata = form.metadata(unix_now()?)?;
    let channel_id = form
        .channel_id
        .clone()
        .ok_or_else(|| AppError::new("choose a channel"))?;
    let subtitles = form
        .subtitles
        .then(|| {
            std::fs::read_to_string(video::subtitles_path(&file)).map_err(|_| {
                AppError::new(
                    "this video has no subtitle file: export it again, or untick the subtitles",
                )
            })
        })
        .transpose()?;
    let previous = {
        let conn = state.conn()?;
        job::save_form(&conn, story_id, &form)?;
        // Without its channel the earlier upload cannot be seen, and the video could go up twice.
        if job::has_unadopted(&conn, story_id)? {
            return Err(AppError::new(
                "this story's earlier upload is not carried over to its channel yet: see the settings",
            ));
        }
        job::load_record(&conn, story_id, &channel_id)?
    };
    let request = job::Request {
        video: &file,
        metadata,
        subtitles,
        again,
    };
    let mut save =
        |record: &job::Record| job::save_record(&*state.conn()?, story_id, &channel_id, record);
    let mut progress = |sent: u64, size: u64| {
        let percent = sent as f64 * 100.0 / size.max(1) as f64;
        let _ = app.emit(PROGRESS_EVENT, ProgressEvent { story_id, percent });
    };
    let api = api(&channel_id)?;
    match job::run(&api, request, previous, cancel, &mut save, &mut progress) {
        Ok(record) => Ok(Some(record.view())),
        Err(error) if error.to_string() == upload::CANCELLED_MESSAGE => {
            let saved = job::load_record(&*state.conn()?, story_id, &channel_id)?;
            Ok(saved.map(|record| record.view()))
        }
        Err(error) => Err(error),
    }
}

/// Uploads an exported video with the form's choices, then its subtitles. A cancel is not an error: the session is
/// kept and the next upload continues it.
#[tauri::command]
pub async fn upload_youtube(
    app: tauri::AppHandle,
    youtube: State<'_, YoutubeState>,
    story_id: i64,
    form: Form,
    again: bool,
) -> Result<Option<View>, AppError> {
    let (_taken, cancel) = youtube.upload.take("another video is being uploaded")?;
    tauri::async_runtime::spawn_blocking(move || upload_story(&app, story_id, form, again, &cancel))
        .await
        .map_err(|e| AppError::new(format!("the upload stopped: {e}")))?
}

#[tauri::command]
pub fn cancel_youtube_upload(youtube: State<'_, YoutubeState>) {
    youtube.upload.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub async fn retry_youtube_captions(
    app: tauri::AppHandle,
    youtube: State<'_, YoutubeState>,
    story_id: i64,
    channel_id: String,
) -> Result<View, AppError> {
    let (_taken, _) = youtube.upload.take("a video is being uploaded")?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let record = job::load_record(&*state.conn()?, story_id, &channel_id)?
            .ok_or_else(|| AppError::new("this story has not been uploaded to this channel"))?;
        let record = job::retry_captions(&api(&channel_id)?, record)?;
        job::save_record(&*state.conn()?, story_id, &channel_id, &record)?;
        Ok(record.view())
    })
    .await
    .map_err(|e| AppError::new(format!("the subtitle upload stopped: {e}")))?
}
