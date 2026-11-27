//! Tauri commands for the YouTube account.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use super::form::Form;
use super::job::{self, View};
use super::upload::{self, Api};
use super::{auth, keychain};
use crate::error::AppError;
use crate::upscale::Resolution;
use crate::{AppState, db, media, video};

/// Relative to the home folder.
const CLIENT_FILE: &str = ".config/audio-books/youtube-client.json";
const CONSENT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

const API: &str = "https://www.googleapis.com";
const PROGRESS_EVENT: &str = "youtube-progress";
const RETRY_BACKOFF: Duration = Duration::from_secs(2);
/// An access token lives for an hour.
const ACCESS_TOKEN_AGE: Duration = Duration::from_secs(50 * 60);

/// Work that runs one at a time and can be cancelled.
#[derive(Default)]
struct Slot {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// Frees the slot when dropped.
struct Taken(Arc<AtomicBool>);

impl Drop for Taken {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Slot {
    fn take(&self, busy: &str) -> Result<(Taken, Arc<AtomicBool>), AppError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(busy));
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok((Taken(Arc::clone(&self.running)), Arc::clone(&self.cancel)))
    }
}

#[derive(Default)]
pub struct YoutubeState {
    sign_in: Slot,
    upload: Slot,
}

pub(crate) fn client_file() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(CLIENT_FILE))
        .ok_or_else(|| AppError::new("HOME is not set"))
}

#[tauri::command]
pub fn youtube_connected() -> Result<bool, AppError> {
    Ok(keychain::load(keychain::SERVICE)?.is_some())
}

#[tauri::command]
pub async fn connect_youtube(
    app: tauri::AppHandle,
    youtube: State<'_, YoutubeState>,
) -> Result<(), AppError> {
    let client = auth::Client::load(&client_file()?)?;
    let (_taken, cancel) = youtube
        .sign_in
        .take("a YouTube sign-in is already waiting for the browser")?;
    tauri::async_runtime::spawn_blocking(move || {
        let open = |url: &str| {
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|e| AppError::new(format!("cannot open the browser: {e}")))
        };
        let token = auth::sign_in(&client, open, &cancel, CONSENT_TIMEOUT)?;
        keychain::save(keychain::SERVICE, &token)
    })
    .await
    .map_err(|e| AppError::new(format!("YouTube sign-in stopped: {e}")))?
}

#[tauri::command]
pub fn cancel_youtube_connect(youtube: State<'_, YoutubeState>) {
    youtube.sign_in.cancel.store(true, Ordering::SeqCst);
}

/// The API with the saved sign-in; an expired one is forgotten so the UI offers to connect again.
fn api() -> Result<Api, AppError> {
    let client = auth::Client::load(&client_file()?)?;
    let saved = keychain::load(keychain::SERVICE)?
        .ok_or_else(|| AppError::new("connect the YouTube account first"))?;
    let cached: RefCell<Option<(String, Instant)>> = RefCell::new(None);
    let token = move || {
        if let Some((token, at)) = cached.borrow().as_ref()
            && at.elapsed() < ACCESS_TOKEN_AGE
        {
            return Ok(token.clone());
        }
        match auth::refresh(&client, &saved)? {
            auth::Refreshed::Access(token) => {
                *cached.borrow_mut() = Some((token.clone(), Instant::now()));
                Ok(token)
            }
            auth::Refreshed::Expired => {
                keychain::delete(keychain::SERVICE)?;
                Err(AppError::new(
                    "the YouTube sign-in has expired or was revoked: connect the account again",
                ))
            }
        }
    };
    Ok(Api {
        base: API.to_owned(),
        token: Box::new(token),
        backoff: RETRY_BACKOFF,
    })
}

#[derive(Serialize)]
pub struct YoutubeInfo {
    connected: bool,
    uploading: bool,
    form: Form,
    videos: Vec<video::VideoFile>,
    upload: Option<View>,
}

#[tauri::command]
pub fn get_youtube(
    state: State<'_, AppState>,
    youtube: State<'_, YoutubeState>,
    story_id: i64,
) -> Result<YoutubeInfo, AppError> {
    let connected = youtube_connected()?;
    let conn = state.conn()?;
    let form = match job::load_form(&conn, story_id)? {
        Some(form) => form,
        None => Form {
            title: db::get_story(&conn, story_id)?.title,
            ..Form::default()
        },
    };
    Ok(YoutubeInfo {
        connected,
        uploading: youtube.upload.running.load(Ordering::SeqCst),
        form,
        videos: video::existing_videos(&media::story_media_dir(&state.workspace, story_id)),
        upload: job::load_record(&conn, story_id)?.map(|record| record.view()),
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

fn unix_now() -> Result<i64, AppError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .map_err(|e| AppError::new(format!("the clock is wrong: {e}")))
}

fn upload_story(
    app: &tauri::AppHandle,
    story_id: i64,
    resolution: Resolution,
    form: Form,
    again: bool,
    cancel: &AtomicBool,
) -> Result<Option<View>, AppError> {
    let state = app.state::<AppState>();
    let media_dir = media::story_media_dir(&state.workspace, story_id);
    let file = video::video_path(&media_dir, resolution);
    let metadata = form.metadata(unix_now()?)?;
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
        job::load_record(&conn, story_id)?
    };
    let request = job::Request {
        video: &file,
        resolution: resolution.label(),
        metadata,
        subtitles,
        again,
    };
    let mut save = |record: &job::Record| job::save_record(&*state.conn()?, story_id, record);
    let mut progress = |sent: u64, size: u64| {
        let percent = sent as f64 * 100.0 / size.max(1) as f64;
        let _ = app.emit(PROGRESS_EVENT, ProgressEvent { story_id, percent });
    };
    match job::run(&api()?, request, previous, cancel, &mut save, &mut progress) {
        Ok(record) => Ok(Some(record.view())),
        Err(error) if error.to_string() == upload::CANCELLED_MESSAGE => {
            Ok(job::load_record(&*state.conn()?, story_id)?.map(|record| record.view()))
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
    resolution: String,
    form: Form,
    again: bool,
) -> Result<Option<View>, AppError> {
    let resolution = Resolution::parse(&resolution)?;
    let (_taken, cancel) = youtube.upload.take("another video is being uploaded")?;
    tauri::async_runtime::spawn_blocking(move || {
        upload_story(&app, story_id, resolution, form, again, &cancel)
    })
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
) -> Result<View, AppError> {
    let (_taken, _) = youtube.upload.take("a video is being uploaded")?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let record = job::load_record(&*state.conn()?, story_id)?
            .ok_or_else(|| AppError::new("this story has not been uploaded"))?;
        let record = job::retry_captions(&api()?, record)?;
        job::save_record(&*state.conn()?, story_id, &record)?;
        Ok(record.view())
    })
    .await
    .map_err(|e| AppError::new(format!("the subtitle upload stopped: {e}")))?
}

/// Forgets the token here first, then asks Google to revoke it.
#[tauri::command]
pub async fn disconnect_youtube() -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(token) = keychain::load(keychain::SERVICE)? else {
            return Ok(());
        };
        keychain::delete(keychain::SERVICE)?;
        let manual = "disconnected here, but Google could not be told: remove the app at \
                      myaccount.google.com/permissions";
        let response = auth::http()
            .post(REVOKE_URL)
            .send_form([("token", token.as_str())])
            .map_err(|e| AppError::new(format!("{manual} ({e})")))?;
        // 400: the token had already expired or been revoked.
        match response.status().as_u16() {
            200 | 400 => Ok(()),
            status => Err(AppError::new(format!("{manual} (HTTP {status})"))),
        }
    })
    .await
    .map_err(|e| AppError::new(format!("disconnecting stopped: {e}")))?
}
