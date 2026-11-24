use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS stories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT NOT NULL,
    text       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS scenes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id     INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    text         TEXT NOT NULL,
    image_prompt TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id      INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    role          TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content       TEXT NOT NULL,
    scenes_changed INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_messages_story ON messages(story_id, id);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_sessions (
    story_id   INTEGER PRIMARY KEY REFERENCES stories(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_scenes_story ON scenes(story_id, position);
";

#[derive(Debug, Clone, Serialize)]
pub struct Story {
    pub id: i64,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scene {
    pub id: i64,
    pub position: i64,
    pub text: String,
    pub image_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub scenes_changed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct NewScene {
    pub text: String,
    pub image_prompt: String,
}

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn init(conn: &Connection) -> Result<(), AppError> {
    // The embedded Claude console may write to this file concurrently.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

pub fn create_story(conn: &Connection, title: &str, text: &str) -> Result<Story, AppError> {
    conn.execute(
        "INSERT INTO stories (title, text) VALUES (?1, ?2)",
        params![title, text],
    )?;
    Ok(Story {
        id: conn.last_insert_rowid(),
        title: title.to_string(),
        text: text.to_string(),
    })
}

pub fn list_stories(conn: &Connection) -> Result<Vec<Story>, AppError> {
    let mut stmt = conn.prepare("SELECT id, title, text FROM stories ORDER BY id DESC")?;
    let rows = stmt.query_map([], |row| {
        Ok(Story {
            id: row.get(0)?,
            title: row.get(1)?,
            text: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_story(conn: &Connection, id: i64) -> Result<Story, AppError> {
    conn.query_row(
        "SELECT id, title, text FROM stories WHERE id = ?1",
        [id],
        |row| {
            Ok(Story {
                id: row.get(0)?,
                title: row.get(1)?,
                text: row.get(2)?,
            })
        },
    )
    .map_err(|_| AppError::new(format!("story {id} not found")))
}

pub fn delete_story(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM stories WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_scenes(conn: &Connection, story_id: i64) -> Result<Vec<Scene>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, position, text, image_prompt FROM scenes
         WHERE story_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(Scene {
            id: row.get(0)?,
            position: row.get(1)?,
            text: row.get(2)?,
            image_prompt: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Atomically replaces all scenes of a story.
pub fn replace_scenes(
    conn: &mut Connection,
    story_id: i64,
    scenes: &[NewScene],
) -> Result<(), AppError> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM scenes WHERE story_id = ?1", [story_id])?;
    for (index, scene) in scenes.iter().enumerate() {
        tx.execute(
            "INSERT INTO scenes (story_id, position, text, image_prompt) VALUES (?1, ?2, ?3, ?4)",
            params![story_id, index as i64, scene.text, scene.image_prompt],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn add_message(
    conn: &Connection,
    story_id: i64,
    role: &str,
    content: &str,
    scenes_changed: bool,
) -> Result<Message, AppError> {
    conn.execute(
        "INSERT INTO messages (story_id, role, content, scenes_changed) VALUES (?1, ?2, ?3, ?4)",
        params![story_id, role, content, scenes_changed],
    )?;
    Ok(Message {
        id: conn.last_insert_rowid(),
        role: role.to_string(),
        content: content.to_string(),
        scenes_changed,
    })
}

pub fn list_messages(conn: &Connection, story_id: i64) -> Result<Vec<Message>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, scenes_changed FROM messages WHERE story_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(Message {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            scenes_changed: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_session(conn: &Connection, story_id: i64) -> Result<Option<String>, AppError> {
    let mut stmt = conn.prepare("SELECT session_id FROM chat_sessions WHERE story_id = ?1")?;
    let mut rows = stmt.query([story_id])?;
    Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
}

pub fn set_session(conn: &Connection, story_id: i64, session_id: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO chat_sessions (story_id, session_id) VALUES (?1, ?2)
         ON CONFLICT(story_id) DO UPDATE SET session_id = excluded.session_id",
        params![story_id, session_id],
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, AppError> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    fn scene(text: &str) -> NewScene {
        NewScene {
            text: text.into(),
            image_prompt: format!("prompt for {text}"),
        }
    }

    #[test]
    fn creates_and_lists_stories_newest_first() {
        let conn = memory_db();
        create_story(&conn, "A", "text a").unwrap();
        create_story(&conn, "B", "text b").unwrap();

        let titles: Vec<_> = list_stories(&conn).unwrap().into_iter().map(|s| s.title).collect();

        assert_eq!(titles, ["B", "A"]);
    }

    #[test]
    fn replace_scenes_overwrites_previous_and_keeps_order() {
        let mut conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        replace_scenes(&mut conn, story.id, &[scene("old")]).unwrap();

        replace_scenes(&mut conn, story.id, &[scene("one"), scene("two")]).unwrap();

        let texts: Vec<_> = list_scenes(&conn, story.id).unwrap().into_iter().map(|s| s.text).collect();
        assert_eq!(texts, ["one", "two"]);
    }

    #[test]
    fn deleting_story_removes_its_scenes() {
        let mut conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        replace_scenes(&mut conn, story.id, &[scene("one")]).unwrap();

        delete_story(&conn, story.id).unwrap();

        assert!(list_scenes(&conn, story.id).unwrap().is_empty());
    }

    #[test]
    fn get_story_errors_when_missing() {
        let conn = memory_db();
        assert!(get_story(&conn, 42).is_err());
    }

    #[test]
    fn messages_keep_order_and_cascade_on_story_delete() {
        let conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        add_message(&conn, story.id, "user", "hi", false).unwrap();
        add_message(&conn, story.id, "assistant", "hello", true).unwrap();

        let messages = list_messages(&conn, story.id).unwrap();
        assert_eq!(messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["hi", "hello"]);
        assert!(messages[1].scenes_changed);

        delete_story(&conn, story.id).unwrap();
        assert!(list_messages(&conn, story.id).unwrap().is_empty());
    }

    #[test]
    fn session_is_upserted_per_story() {
        let conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        assert_eq!(get_session(&conn, story.id).unwrap(), None);

        set_session(&conn, story.id, "s1").unwrap();
        set_session(&conn, story.id, "s2").unwrap();

        assert_eq!(get_session(&conn, story.id).unwrap().as_deref(), Some("s2"));
    }

    #[test]
    fn settings_are_upserted() {
        let conn = memory_db();
        assert_eq!(get_setting(&conn, "model").unwrap(), None);

        set_setting(&conn, "model", "a").unwrap();
        set_setting(&conn, "model", "b").unwrap();

        assert_eq!(get_setting(&conn, "model").unwrap().as_deref(), Some("b"));
    }
}
