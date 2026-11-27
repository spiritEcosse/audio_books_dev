//! Tauri commands for the connected channels, and the signed-in API of each.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

use super::auth;
use super::channels::{self, Channel};
use super::commands::YoutubeState;
use super::keychain::{self, Keychain, LEGACY_ACCOUNT, Tokens};
use super::upload::Api;
use crate::AppState;
use crate::error::AppError;

/// Relative to the home folder.
const CLIENT_FILE: &str = ".config/audio-books/youtube-client.json";
const CONSENT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const API: &str = "https://www.googleapis.com";
const RETRY_BACKOFF: Duration = Duration::from_secs(2);
/// An access token lives for an hour.
const ACCESS_TOKEN_AGE: Duration = Duration::from_secs(50 * 60);
const TOKENS: Keychain = Keychain(keychain::SERVICE);
const EXPIRED: &str =
    "the YouTube sign-in has expired or was revoked: reconnect the channel in the settings";

pub(crate) fn client_file() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(CLIENT_FILE))
        .ok_or_else(|| AppError::new("HOME is not set"))
}

fn with_token(token: Box<dyn Fn() -> Result<String, AppError>>) -> Api {
    Api {
        base: API.to_owned(),
        token,
        backoff: RETRY_BACKOFF,
    }
}

/// The channel a refresh token signs in to; `None` when the token has expired.
fn identify(client: &auth::Client, refresh_token: &str) -> Result<Option<Channel>, AppError> {
    match auth::refresh(client, refresh_token)? {
        auth::Refreshed::Access(access) => {
            channels::mine(&with_token(Box::new(move || Ok(access.clone())))).map(Some)
        }
        auth::Refreshed::Expired => Ok(None),
    }
}

/// The API with the channel's sign-in; an expired one is forgotten so that the settings offer to reconnect.
pub(super) fn api(channel_id: &str) -> Result<Api, AppError> {
    let client = auth::Client::load(&client_file()?)?;
    let account = keychain::account(channel_id);
    let saved = TOKENS
        .load(&account)?
        .ok_or_else(|| AppError::new("connect this channel in the settings first"))?;
    let cached: RefCell<Option<(String, Instant)>> = RefCell::new(None);
    Ok(with_token(Box::new(move || {
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
                // The channel may have been reconnected since: only the token that failed is removed.
                if TOKENS.load(&account)?.as_deref() == Some(saved.as_str()) {
                    TOKENS.delete(&account)?;
                }
                Err(AppError::new(EXPIRED))
            }
        }
    })))
}

/// Carries the single account of an older build over to its channel; needs the network once.
fn migrate(state: &AppState) -> Result<(), AppError> {
    let Some(token) = TOKENS.load(LEGACY_ACCOUNT)? else {
        return Ok(());
    };
    let client = auth::Client::load(&client_file()?)?;
    let channel = identify(&client, &token)?;
    channels::adopt_legacy(&*state.conn()?, &TOKENS, channel.as_ref())
}

#[derive(Serialize)]
pub struct ChannelView {
    #[serde(flatten)]
    channel: Channel,
    /// False when the sign-in expired and the channel has to be reconnected.
    connected: bool,
}

#[derive(Serialize)]
pub struct Channels {
    channels: Vec<ChannelView>,
    /// Why the account of an older version could not be carried over yet.
    warning: Option<String>,
}

#[tauri::command]
pub async fn youtube_channels(app: tauri::AppHandle) -> Result<Channels, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let warning = migrate(&state).err().map(|error| {
            format!("the account connected earlier could not be carried over: {error}")
        });
        let listed = channels::list(&*state.conn()?)?;
        let channels = listed
            .into_iter()
            .map(|channel| {
                let connected = TOKENS.load(&keychain::account(&channel.id))?.is_some();
                Ok(ChannelView { channel, connected })
            })
            .collect::<Result<_, AppError>>()?;
        Ok(Channels { channels, warning })
    })
    .await
    .map_err(|e| AppError::new(format!("listing the channels stopped: {e}")))?
}

/// Signs in through the browser and adds the chosen channel, or refreshes it if it is connected already.
#[tauri::command]
pub async fn connect_youtube(
    app: tauri::AppHandle,
    youtube: State<'_, YoutubeState>,
) -> Result<Channel, AppError> {
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
        let channel = identify(&client, &token)?
            .ok_or_else(|| AppError::new("Google refused the new sign-in: try again"))?;
        let state = app.state::<AppState>();
        channels::connect(&*state.conn()?, &TOKENS, &channel, &token)?;
        Ok(channel)
    })
    .await
    .map_err(|e| AppError::new(format!("YouTube sign-in stopped: {e}")))?
}

#[tauri::command]
pub fn cancel_youtube_connect(youtube: State<'_, YoutubeState>) {
    youtube.sign_in.cancel.store(true, Ordering::SeqCst);
}

/// Forgets the channel here first, then asks Google to revoke its token.
#[tauri::command]
pub async fn disconnect_youtube(
    app: tauri::AppHandle,
    youtube: State<'_, YoutubeState>,
    channel_id: String,
) -> Result<(), AppError> {
    let (taken, _) = youtube
        .upload
        .take("wait for the upload to finish before disconnecting")?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let removed = channels::remove(&*state.conn()?, &TOKENS, &channel_id);
        // Freed before Google is told, which can be slow.
        drop(taken);
        let Some(token) = removed? else {
            return Ok(());
        };
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

/// Gives up on carrying over the single account of an older build.
#[tauri::command]
pub fn forget_legacy_youtube(state: State<'_, AppState>) -> Result<(), AppError> {
    channels::adopt_legacy(&*state.conn()?, &TOKENS, None)
}
