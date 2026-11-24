mod claude;
mod db;
mod error;
mod pty;

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};

use db::{Scene, Story};
use error::AppError;

struct AppState {
    db: Mutex<Connection>,
    workspace: std::path::PathBuf,
}

impl AppState {
    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        self.db.lock().map_err(|_| AppError::new("database lock poisoned"))
    }
}

#[derive(serde::Serialize)]
struct StoryDetail {
    story: Story,
    scenes: Vec<Scene>,
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
    let conn = state.conn()?;
    let conn = &*conn;
    Ok(StoryDetail {
        story: db::get_story(conn, id)?,
        scenes: db::list_scenes(conn, id)?,
    })
}

#[tauri::command]
async fn split_story(state: State<'_, AppState>, id: i64) -> Result<Vec<Scene>, AppError> {
    let story = db::get_story(&*state.conn()?, id)?;
    let workspace = state.workspace.clone();

    let new_scenes = tauri::async_runtime::spawn_blocking(move || {
        claude::split_into_scenes(&workspace, &story.text)
    })
    .await
    .map_err(|e| AppError::new(format!("split task failed: {e}")))??;

    let mut conn = state.conn()?;
    db::replace_scenes(&mut conn, id, &new_scenes)?;
    db::list_scenes(&conn, id)
}

#[tauri::command]
fn pty_spawn(
    app: AppHandle,
    state: State<'_, AppState>,
    pty: State<'_, pty::PtyState>,
    rows: u16,
    cols: u16,
    output: Channel<InvokeResponseBody>,
) -> Result<(), AppError> {
    pty.spawn(app, &state.workspace, rows, cols, output)
}

#[tauri::command]
fn pty_write(pty: State<'_, pty::PtyState>, data: String) -> Result<(), AppError> {
    pty.write(&data)
}

#[tauri::command]
fn pty_resize(pty: State<'_, pty::PtyState>, rows: u16, cols: u16) -> Result<(), AppError> {
    pty.resize(rows, cols)
}

#[tauri::command]
fn pty_kill(pty: State<'_, pty::PtyState>) -> Result<(), AppError> {
    pty.kill()
}

const WORKSPACE_GUIDE: &str = include_str!("../workspace_CLAUDE.md");

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let guide = data_dir.join("CLAUDE.md");
            // App-managed file: always refreshed so guidance updates ship with the app.
            std::fs::write(&guide, WORKSPACE_GUIDE)?;

            let conn = Connection::open(data_dir.join("audiobooks.db"))?;
            db::init(&conn)?;
            app.manage(AppState { db: Mutex::new(conn), workspace: data_dir });
            app.manage(pty::PtyState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stories,
            create_story,
            delete_story,
            get_story,
            split_story,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
