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
CREATE TABLE IF NOT EXISTS proposals (
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    text       TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'accepted', 'rejected'))
);
CREATE TABLE IF NOT EXISTS revisions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id   INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('story_text', 'scenes')),
    payload    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_revisions_story ON revisions(story_id, id);
CREATE TABLE IF NOT EXISTS pending_notes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    text     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS story_settings (
    story_id       INTEGER PRIMARY KEY REFERENCES stories(id) ON DELETE CASCADE,
    target_minutes INTEGER NOT NULL,
    age_group      TEXT NOT NULL,
    profile        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_sessions (
    story_id   INTEGER PRIMARY KEY REFERENCES stories(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_scenes_story ON scenes(story_id, position);
CREATE TABLE IF NOT EXISTS story_media (
    story_id    INTEGER PRIMARY KEY REFERENCES stories(id) ON DELETE CASCADE,
    image_model TEXT,
    voice_id    TEXT
);
-- Keyed by scene position, not scene id: replace_scenes recreates every scene row.
-- 'source' is the scene text / image prompt the asset was requested for (staleness check).
CREATE TABLE IF NOT EXISTS scene_assets (
    story_id   INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('image', 'audio')),
    status     TEXT NOT NULL CHECK (status IN ('requested', 'ready', 'failed')),
    file_path  TEXT,
    source     TEXT NOT NULL,
    model_id   TEXT NOT NULL,
    voice_id   TEXT,
    credits    REAL,
    estimate_credits REAL,
    started_at TEXT,
    error      TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (story_id, position, kind)
);
-- YouTube: the upload form of a story as JSON (`youtube/job.rs`). `upload` held the only upload of older builds;
-- it is moved to `youtube_uploads` once the channel is known.
CREATE TABLE IF NOT EXISTS youtube (
    story_id INTEGER PRIMARY KEY REFERENCES stories(id) ON DELETE CASCADE,
    form     TEXT,
    upload   TEXT
);
-- No foreign key to the channels: the records outlive a disconnect.
CREATE TABLE IF NOT EXISTS youtube_uploads (
    story_id   INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    upload     TEXT NOT NULL,
    PRIMARY KEY (story_id, channel_id)
);
-- Names and pictures only: the tokens are in the Keychain.
CREATE TABLE IF NOT EXISTS youtube_channels (
    id        TEXT PRIMARY KEY,
    title     TEXT NOT NULL,
    thumbnail TEXT
);
CREATE TABLE IF NOT EXISTS scene_references (
    story_id  INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    file_path TEXT NOT NULL,
    PRIMARY KEY (story_id, position)
);
CREATE TABLE IF NOT EXISTS story_characters (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id  INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    name      TEXT NOT NULL,
    file_path TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS scene_character_off (
    story_id     INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    character_id INTEGER NOT NULL REFERENCES story_characters(id) ON DELETE CASCADE,
    PRIMARY KEY (story_id, position, character_id)
);
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
pub struct Proposal {
    pub text: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub scenes_changed: bool,
    pub proposal: Option<Proposal>,
}

#[derive(Debug, Clone)]
pub struct Revision {
    pub id: i64,
    pub story_id: i64,
    pub kind: String,
    pub payload: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub text: String,
}

pub const AGE_GROUPS: [&str; 4] = ["up_to_5", "up_to_10", "up_to_15", "adult"];
pub const DEFAULT_AGE_GROUP: &str = "up_to_10";
pub const DEFAULT_TARGET_MINUTES: i64 = 10;
pub const MIN_TARGET_MINUTES: i64 = 1;
pub const MAX_TARGET_MINUTES: i64 = 600;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StorySettings {
    pub target_minutes: i64,
    pub age_group: String,
    pub profile: String,
}

const MAX_REVISIONS_LISTED: i64 = 100;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct NewScene {
    pub text: String,
    pub image_prompt: String,
}

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Adds a column to a table created by an older build (there is no migration framework).
/// Callers pass compile-time constants only.
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    let exists = conn
        .prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"
        ))?
        .exists([column])?;
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

pub fn init(conn: &Connection) -> Result<(), AppError> {
    // The embedded Claude console may write to this file concurrently.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch(SCHEMA)?;
    ensure_column(conn, "scene_assets", "estimate_credits", "REAL")?;
    ensure_column(conn, "scene_assets", "started_at", "TEXT")?;
    ensure_column(conn, "scene_assets", "session_id", "TEXT")?;
    ensure_column(conn, "scene_assets", "flow_id", "TEXT")?;
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
        proposal: None,
    })
}

pub fn list_messages(conn: &Connection, story_id: i64) -> Result<Vec<Message>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.role, m.content, m.scenes_changed, p.text, p.state
         FROM messages m LEFT JOIN proposals p ON p.message_id = m.id
         WHERE m.story_id = ?1 ORDER BY m.id",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        let text: Option<String> = row.get(4)?;
        let state: Option<String> = row.get(5)?;
        Ok(Message {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            scenes_changed: row.get(3)?,
            proposal: text
                .zip(state)
                .map(|(text, state)| Proposal { text, state }),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn add_proposal(conn: &Connection, message_id: i64, text: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO proposals (message_id, text) VALUES (?1, ?2)",
        params![message_id, text],
    )?;
    Ok(())
}

/// Returns the story a proposal belongs to together with the proposal itself.
pub fn get_proposal(conn: &Connection, message_id: i64) -> Result<(i64, Proposal), AppError> {
    conn.query_row(
        "SELECT m.story_id, p.text, p.state FROM proposals p
         JOIN messages m ON m.id = p.message_id WHERE p.message_id = ?1",
        [message_id],
        |row| {
            Ok((
                row.get(0)?,
                Proposal {
                    text: row.get(1)?,
                    state: row.get(2)?,
                },
            ))
        },
    )
    .map_err(|_| AppError::new(format!("proposal for message {message_id} not found")))
}

pub fn set_proposal_state(conn: &Connection, message_id: i64, state: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE proposals SET state = ?2 WHERE message_id = ?1",
        params![message_id, state],
    )?;
    Ok(())
}

pub fn update_story_text(conn: &Connection, story_id: i64, text: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE stories SET text = ?2 WHERE id = ?1",
        params![story_id, text],
    )?;
    Ok(())
}

pub fn add_revision(
    conn: &Connection,
    story_id: i64,
    kind: &str,
    payload: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO revisions (story_id, kind, payload) VALUES (?1, ?2, ?3)",
        params![story_id, kind, payload],
    )?;
    Ok(())
}

fn revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Revision> {
    Ok(Revision {
        id: row.get(0)?,
        story_id: row.get(1)?,
        kind: row.get(2)?,
        payload: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// Newest first.
pub fn list_revisions(conn: &Connection, story_id: i64) -> Result<Vec<Revision>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, story_id, kind, payload, created_at FROM revisions
         WHERE story_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![story_id, MAX_REVISIONS_LISTED], revision_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_revision(conn: &Connection, id: i64) -> Result<Revision, AppError> {
    conn.query_row(
        "SELECT id, story_id, kind, payload, created_at FROM revisions WHERE id = ?1",
        [id],
        revision_from_row,
    )
    .map_err(|_| AppError::new(format!("revision {id} not found")))
}

pub fn add_note(conn: &Connection, story_id: i64, text: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO pending_notes (story_id, text) VALUES (?1, ?2)",
        params![story_id, text],
    )?;
    Ok(())
}

pub fn list_notes(conn: &Connection, story_id: i64) -> Result<Vec<Note>, AppError> {
    let mut stmt =
        conn.prepare("SELECT id, text FROM pending_notes WHERE story_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(Note {
            id: row.get(0)?,
            text: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn delete_notes(conn: &Connection, ids: &[i64]) -> Result<(), AppError> {
    for id in ids {
        conn.execute("DELETE FROM pending_notes WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// Per-story settings; stories without saved settings get the defaults.
pub fn get_story_settings(
    conn: &Connection,
    story_id: i64,
    default_profile: &str,
) -> Result<StorySettings, AppError> {
    let mut stmt = conn.prepare(
        "SELECT target_minutes, age_group, profile FROM story_settings WHERE story_id = ?1",
    )?;
    let mut rows = stmt.query([story_id])?;
    let saved = rows
        .next()?
        .map(|row| -> rusqlite::Result<StorySettings> {
            Ok(StorySettings {
                target_minutes: row.get(0)?,
                age_group: row.get(1)?,
                profile: row.get(2)?,
            })
        })
        .transpose()?;
    Ok(saved.unwrap_or_else(|| StorySettings {
        target_minutes: DEFAULT_TARGET_MINUTES,
        age_group: DEFAULT_AGE_GROUP.to_owned(),
        profile: default_profile.to_owned(),
    }))
}

/// Validates minutes and age group; the caller validates the profile id.
pub fn set_story_settings(
    conn: &Connection,
    story_id: i64,
    settings: &StorySettings,
) -> Result<(), AppError> {
    if !(MIN_TARGET_MINUTES..=MAX_TARGET_MINUTES).contains(&settings.target_minutes) {
        return Err(AppError::new(format!(
            "target length must be {MIN_TARGET_MINUTES}-{MAX_TARGET_MINUTES} minutes"
        )));
    }
    if !AGE_GROUPS.contains(&settings.age_group.as_str()) {
        return Err(AppError::new(format!(
            "unknown audience: {}",
            settings.age_group
        )));
    }
    conn.execute(
        "INSERT INTO story_settings (story_id, target_minutes, age_group, profile) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(story_id) DO UPDATE SET target_minutes = excluded.target_minutes,
            age_group = excluded.age_group, profile = excluded.profile",
        params![story_id, settings.target_minutes, settings.age_group, settings.profile],
    )?;
    Ok(())
}

/// Forgets the Claude session so the next turn rebuilds context from history
/// (a resumed session keeps its old system prompt).
pub fn delete_session(conn: &Connection, story_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM chat_sessions WHERE story_id = ?1", [story_id])?;
    Ok(())
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

        let titles: Vec<_> = list_stories(&conn)
            .unwrap()
            .into_iter()
            .map(|s| s.title)
            .collect();

        assert_eq!(titles, ["B", "A"]);
    }

    #[test]
    fn replace_scenes_overwrites_previous_and_keeps_order() {
        let mut conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        replace_scenes(&mut conn, story.id, &[scene("old")]).unwrap();

        replace_scenes(&mut conn, story.id, &[scene("one"), scene("two")]).unwrap();

        let texts: Vec<_> = list_scenes(&conn, story.id)
            .unwrap()
            .into_iter()
            .map(|s| s.text)
            .collect();
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
        assert_eq!(
            messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            ["hi", "hello"]
        );
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
    fn ensure_column_adds_a_missing_column_once() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER)").unwrap();
        ensure_column(&conn, "t", "b", "REAL").unwrap();
        ensure_column(&conn, "t", "b", "REAL").unwrap();
        conn.execute("INSERT INTO t (a, b) VALUES (1, 2.5)", [])
            .unwrap();
        let b: f64 = conn
            .query_row("SELECT b FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(b, 2.5);
    }

    #[test]
    fn settings_are_upserted() {
        let conn = memory_db();
        assert_eq!(get_setting(&conn, "model").unwrap(), None);

        set_setting(&conn, "model", "a").unwrap();
        set_setting(&conn, "model", "b").unwrap();

        assert_eq!(get_setting(&conn, "model").unwrap().as_deref(), Some("b"));
    }

    #[test]
    fn story_settings_default_then_override_and_validate() {
        let conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        let defaults = get_story_settings(&conn, story.id, "fairy_tale").unwrap();
        assert_eq!(
            (defaults.target_minutes, defaults.age_group.as_str()),
            (10, "up_to_10")
        );

        let custom = StorySettings {
            target_minutes: 5,
            age_group: "up_to_5".into(),
            profile: "novel".into(),
        };
        set_story_settings(&conn, story.id, &custom).unwrap();
        assert_eq!(
            get_story_settings(&conn, story.id, "fairy_tale").unwrap(),
            custom
        );

        let too_long = StorySettings {
            target_minutes: 9999,
            ..custom.clone()
        };
        assert!(set_story_settings(&conn, story.id, &too_long).is_err());
        let bad_age = StorySettings {
            age_group: "toddler".into(),
            ..custom
        };
        assert!(set_story_settings(&conn, story.id, &bad_age).is_err());
    }

    #[test]
    fn delete_session_forgets_it() {
        let conn = memory_db();
        let story = create_story(&conn, "A", "t").unwrap();
        set_session(&conn, story.id, "s1").unwrap();
        delete_session(&conn, story.id).unwrap();
        assert_eq!(get_session(&conn, story.id).unwrap(), None);
    }
}
