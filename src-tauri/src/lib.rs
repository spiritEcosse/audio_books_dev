mod claude;
mod db;
mod error;

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{Manager, State};

use db::{Scene, Story};
use error::AppError;

struct AppState {
    db: Mutex<Connection>,
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
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| AppError::new("ANTHROPIC_API_KEY is not set (add it to .env)"))?;
    let story = db::get_story(&*state.conn()?, id)?;

    let new_scenes = claude::split_into_scenes(&api_key, &story.text).await?;

    let mut conn = state.conn()?;
    db::replace_scenes(&mut conn, id, &new_scenes)?;
    db::list_scenes(&conn, id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Dev: finds .env in a parent of the cwd. Release: falls back to the app data dir.
    let _ = dotenvy::dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let _ = dotenvy::from_path(data_dir.join(".env"));

            let conn = Connection::open(data_dir.join("audiobooks.db"))?;
            db::init(&conn)?;
            app.manage(AppState { db: Mutex::new(conn) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stories,
            create_story,
            delete_story,
            get_story,
            split_story
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
