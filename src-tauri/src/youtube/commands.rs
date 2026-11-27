//! Tauri commands for the YouTube account.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::State;
use tauri_plugin_opener::OpenerExt;

use super::{auth, keychain};
use crate::error::AppError;

/// Relative to the home folder.
const CLIENT_FILE: &str = ".config/audio-books/youtube-client.json";
const CONSENT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// One sign-in at a time, which can be cancelled.
#[derive(Default)]
pub struct SignIn {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
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
    sign_in: State<'_, SignIn>,
) -> Result<(), AppError> {
    let client = auth::Client::load(&client_file()?)?;
    if sign_in.running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "a YouTube sign-in is already waiting for the browser",
        ));
    }
    sign_in.cancel.store(false, Ordering::SeqCst);
    let cancel = Arc::clone(&sign_in.cancel);
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let open = |url: &str| {
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|e| AppError::new(format!("cannot open the browser: {e}")))
        };
        let token = auth::sign_in(&client, open, &cancel, CONSENT_TIMEOUT)?;
        keychain::save(keychain::SERVICE, &token)
    })
    .await;
    sign_in.running.store(false, Ordering::SeqCst);
    outcome.map_err(|e| AppError::new(format!("YouTube sign-in stopped: {e}")))?
}

#[tauri::command]
pub fn cancel_youtube_connect(sign_in: State<'_, SignIn>) {
    sign_in.cancel.store(true, Ordering::SeqCst);
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
