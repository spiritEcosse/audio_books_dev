mod chat;
mod claude;
mod db;
mod error;

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::{Manager, State};

use db::{Message, Scene, Story};
use error::AppError;

struct AppState {
    db: Mutex<Connection>,
    workspace: PathBuf,
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

#[derive(serde::Serialize)]
struct StoryDetail {
    story: Story,
    scenes: Vec<Scene>,
    messages: Vec<Message>,
}

fn load_detail(conn: &Connection, id: i64) -> Result<StoryDetail, AppError> {
    Ok(StoryDetail {
        story: db::get_story(conn, id)?,
        scenes: db::list_scenes(conn, id)?,
        messages: db::list_messages(conn, id)?,
    })
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

    let (story, scenes, history, session_id, model) = {
        let conn = state.conn()?;
        let story = db::get_story(&conn, id)?;
        let scenes = db::list_scenes(&conn, id)?;
        let history = db::list_messages(&conn, id)?;
        let session_id = db::get_session(&conn, id)?;
        let model = selected_model(&conn)?;
        db::add_message(&conn, id, "user", &text, false)?;
        (story, scenes, history, session_id, model)
    };

    let workspace = state.workspace.clone();
    let turn = tauri::async_runtime::spawn_blocking(move || {
        chat::run_turn(&workspace, &story, &scenes, &history, session_id.as_deref(), &model, &text)
    })
    .await
    .map_err(|e| AppError::new(format!("chat task failed: {e}")))??;

    let mut conn = state.conn()?;
    if let Some(session_id) = &turn.session_id {
        db::set_session(&conn, id, session_id)?;
    }
    if let Some(new_scenes) = &turn.scenes {
        db::replace_scenes(&mut conn, id, new_scenes)?;
    }
    db::add_message(&conn, id, "assistant", &turn.reply, turn.scenes.is_some())?;
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

            let conn = Connection::open(data_dir.join("audiobooks.db"))?;
            db::init(&conn)?;
            app.manage(AppState { db: Mutex::new(conn), workspace: data_dir });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stories,
            create_story,
            delete_story,
            get_story,
            get_models,
            set_model,
            send_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
