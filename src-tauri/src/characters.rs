//! Character references: pictures of a story's recurring characters (the hero, say) that are sent to the image
//! model with every scene they appear in, so a character looks the same from scene to scene. A character applies
//! to every scene unless it is switched off for that scene, so adding one needs no per-scene setup.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::AppError;
use crate::{db, media};

/// More references blur the result and cost upload time; a bedtime story rarely needs more heroes than this.
pub const MAX_CHARACTERS: usize = 4;
const MAX_NAME_CHARS: usize = 60;
const CHARACTER_DIR: &str = "characters";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Character {
    pub id: i64,
    pub name: String,
    pub file_path: String,
}

/// A scene one character is switched off for.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SceneCharacterOff {
    pub position: i64,
    pub character_id: i64,
}

pub fn list(conn: &Connection, story_id: i64) -> Result<Vec<Character>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, file_path FROM story_characters WHERE story_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(Character {
            id: row.get(0)?,
            name: row.get(1)?,
            file_path: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_off(conn: &Connection, story_id: i64) -> Result<Vec<SceneCharacterOff>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT position, character_id FROM scene_character_off WHERE story_id = ?1 ORDER BY position, character_id",
    )?;
    let rows = stmt.query_map([story_id], |row| {
        Ok(SceneCharacterOff {
            position: row.get(0)?,
            character_id: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn save_file(
    workspace: &Path,
    story_id: i64,
    file_name: &str,
    bytes: &[u8],
) -> Result<PathBuf, AppError> {
    let extension = media::reference_extension(file_name)?;
    if bytes.is_empty() || bytes.len() > media::MAX_REFERENCE_BYTES {
        return Err(AppError::new(
            "character image must be between 1 byte and 10 MB",
        ));
    }
    let dir = media::story_media_dir(workspace, story_id).join(CHARACTER_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::new(format!("cannot create {}: {e}", dir.display())))?;
    // A new name every time: the webview caches images by URL.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("character-{stamp}.{extension}"));
    std::fs::write(&path, bytes)
        .map_err(|e| AppError::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

/// Position used to name the files of generated characters (`scene--1-image-v1.png` style); no scene has it.
pub const GENERATION_POSITION: i64 = -1;
const MAX_PROMPT_CHARS: usize = 1000;

/// Checks a new character's name, the story and the room left, before anything is saved or paid for.
fn check_new(conn: &Connection, story_id: i64, name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::new(format!(
            "a character name must be 1 to {MAX_NAME_CHARS} characters"
        )));
    }
    db::get_story(conn, story_id)?;
    if list(conn, story_id)?.len() >= MAX_CHARACTERS {
        return Err(AppError::new(format!(
            "a story can have at most {MAX_CHARACTERS} characters"
        )));
    }
    Ok(name.to_owned())
}

fn insert_row(
    conn: &Connection,
    story_id: i64,
    name: &str,
    path: &Path,
) -> Result<Character, AppError> {
    let file_path = path.display().to_string();
    conn.execute(
        "INSERT INTO story_characters (story_id, name, file_path) VALUES (?1, ?2, ?3)",
        params![story_id, name, file_path],
    )?;
    Ok(Character {
        id: conn.last_insert_rowid(),
        name: name.to_owned(),
        file_path,
    })
}

/// Adds a character with its picture to the story.
pub fn add(
    conn: &Connection,
    workspace: &Path,
    story_id: i64,
    name: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<Character, AppError> {
    let name = check_new(conn, story_id, name)?;
    let path = save_file(workspace, story_id, file_name, bytes)?;
    insert_row(conn, story_id, &name, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&path);
    })
}

/// Validates a character that is about to be generated; returns the trimmed name and prompt.
pub fn prepare_generation(
    conn: &Connection,
    story_id: i64,
    name: &str,
    prompt: &str,
) -> Result<(String, String), AppError> {
    let name = check_new(conn, story_id, name)?;
    let prompt = prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(AppError::new(format!(
            "describe the character in 1 to {MAX_PROMPT_CHARS} characters"
        )));
    }
    Ok((name, prompt.to_owned()))
}

/// The folder a story's character pictures live in (uploaded and generated).
pub fn character_dir(workspace: &Path, story_id: i64) -> PathBuf {
    media::story_media_dir(workspace, story_id).join(CHARACTER_DIR)
}

/// Registers a picture that was generated into the character folder: it gets the usual unique name and becomes
/// the character. The credits are already spent, so on failure the file is left where it is.
pub fn adopt_generated(
    conn: &Connection,
    story_id: i64,
    name: &str,
    generated: &Path,
) -> Result<Character, AppError> {
    let extension = generated
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let target = generated.with_file_name(format!("character-{stamp}.{extension}"));
    std::fs::rename(generated, &target)
        .map_err(|e| AppError::new(format!("cannot move {}: {e}", generated.display())))?;
    insert_row(conn, story_id, name, &target)
}

/// Removes a character; returns its picture so the caller can delete the file.
pub fn remove(
    conn: &Connection,
    story_id: i64,
    character_id: i64,
) -> Result<Option<String>, AppError> {
    let file: Option<String> = conn
        .query_row(
            "SELECT file_path FROM story_characters WHERE id = ?1 AND story_id = ?2",
            params![character_id, story_id],
            |row| row.get(0),
        )
        .optional()?;
    conn.execute(
        "DELETE FROM scene_character_off WHERE character_id = ?1 AND story_id = ?2",
        params![character_id, story_id],
    )?;
    conn.execute(
        "DELETE FROM story_characters WHERE id = ?1 AND story_id = ?2",
        params![character_id, story_id],
    )?;
    Ok(file)
}

/// Switches a character on or off for one scene.
pub fn set_in_scene(
    conn: &Connection,
    story_id: i64,
    position: i64,
    character_id: i64,
    included: bool,
) -> Result<(), AppError> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM story_characters WHERE id = ?1 AND story_id = ?2",
            params![character_id, story_id],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(AppError::new(format!("character {character_id} not found")));
    }
    if included {
        conn.execute(
            "DELETE FROM scene_character_off WHERE story_id = ?1 AND position = ?2 AND character_id = ?3",
            params![story_id, position, character_id],
        )?;
    } else {
        conn.execute(
            "INSERT OR IGNORE INTO scene_character_off (story_id, position, character_id) VALUES (?1, ?2, ?3)",
            params![story_id, position, character_id],
        )?;
    }
    Ok(())
}

/// The pictures of the characters that appear in a scene, in the order they were added.
pub fn files_for_scene(
    conn: &Connection,
    story_id: i64,
    position: i64,
) -> Result<Vec<PathBuf>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT c.file_path FROM story_characters c
         WHERE c.story_id = ?1
           AND NOT EXISTS (SELECT 1 FROM scene_character_off o
                           WHERE o.story_id = c.story_id AND o.position = ?2 AND o.character_id = c.id)
         ORDER BY c.id",
    )?;
    let rows = stmt.query_map(params![story_id, position], |row| row.get::<_, String>(0))?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(PathBuf::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn setup() -> (Connection, i64, PathBuf) {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        // One folder per test: they run in parallel and each setup wipes its folder first.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "audiobooks-test-characters-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (conn, story.id, dir)
    }

    #[test]
    fn an_added_character_is_listed_and_its_file_is_on_disk() {
        let (conn, story, dir) = setup();
        let hero = add(&conn, &dir, story, "  Tembo ", "tembo.PNG", b"png bytes").unwrap();
        assert_eq!(list(&conn, story).unwrap(), [hero.clone()]);
        assert_eq!(hero.name, "Tembo", "the name is trimmed");
        assert_eq!(std::fs::read(&hero.file_path).unwrap(), b"png bytes");
        assert!(hero.file_path.ends_with(".png"));
    }

    #[test]
    fn bad_input_is_rejected_and_leaves_no_character() {
        let (conn, story, dir) = setup();
        assert!(
            add(&conn, &dir, story, "  ", "a.png", b"x").is_err(),
            "empty name"
        );
        assert!(
            add(
                &conn,
                &dir,
                story,
                &"n".repeat(MAX_NAME_CHARS + 1),
                "a.png",
                b"x"
            )
            .is_err(),
            "long name"
        );
        assert!(
            add(&conn, &dir, story, "Tembo", "a.exe", b"x").is_err(),
            "not an image"
        );
        assert!(
            add(&conn, &dir, story, "Tembo", "a.png", b"").is_err(),
            "empty file"
        );
        assert!(
            add(&conn, &dir, 999, "Tembo", "a.png", b"x").is_err(),
            "unknown story"
        );
        assert!(list(&conn, story).unwrap().is_empty());
    }

    #[test]
    fn generation_is_validated_before_any_credits_are_spent() {
        let (conn, story, dir) = setup();
        assert_eq!(
            prepare_generation(&conn, story, " Tembo ", " a small elephant ").unwrap(),
            ("Tembo".to_owned(), "a small elephant".to_owned())
        );
        assert!(
            prepare_generation(&conn, story, "", "a small elephant").is_err(),
            "no name"
        );
        assert!(
            prepare_generation(&conn, story, "Tembo", "  ").is_err(),
            "no description"
        );
        assert!(
            prepare_generation(&conn, story, "Tembo", &"d".repeat(MAX_PROMPT_CHARS + 1)).is_err()
        );
        assert!(
            prepare_generation(&conn, 999, "Tembo", "x").is_err(),
            "unknown story"
        );
        for index in 0..MAX_CHARACTERS {
            add(&conn, &dir, story, &format!("hero {index}"), "a.png", b"x").unwrap();
        }
        assert!(
            prepare_generation(&conn, story, "Tembo", "x").is_err(),
            "no room left"
        );
    }

    #[test]
    fn a_generated_picture_becomes_a_character_under_a_unique_file_name() {
        let (conn, story, dir) = setup();
        let folder = character_dir(&dir, story);
        std::fs::create_dir_all(&folder).unwrap();
        let generated = folder.join("scene--1-image-v1.png");
        std::fs::write(&generated, b"generated").unwrap();

        let hero = adopt_generated(&conn, story, "Tembo", &generated).unwrap();

        assert!(!generated.exists(), "the file was renamed, not copied");
        assert_eq!(std::fs::read(&hero.file_path).unwrap(), b"generated");
        assert!(hero.file_path.contains("character-") && hero.file_path.ends_with(".png"));
        assert_eq!(list(&conn, story).unwrap(), [hero]);
    }

    #[test]
    fn a_story_holds_at_most_the_maximum_number_of_characters() {
        let (conn, story, dir) = setup();
        for index in 0..MAX_CHARACTERS {
            add(&conn, &dir, story, &format!("hero {index}"), "a.png", b"x").unwrap();
        }
        assert!(add(&conn, &dir, story, "one too many", "a.png", b"x").is_err());
    }

    #[test]
    fn a_character_applies_to_every_scene_until_it_is_switched_off_for_one() {
        let (conn, story, dir) = setup();
        let hero = add(&conn, &dir, story, "Tembo", "a.png", b"x").unwrap();
        let owl = add(&conn, &dir, story, "Owl", "b.png", b"y").unwrap();
        let both = [
            PathBuf::from(&hero.file_path),
            PathBuf::from(&owl.file_path),
        ];
        assert_eq!(files_for_scene(&conn, story, 0).unwrap(), both);

        set_in_scene(&conn, story, 0, hero.id, false).unwrap();
        set_in_scene(&conn, story, 0, hero.id, false).unwrap();
        assert_eq!(
            files_for_scene(&conn, story, 0).unwrap(),
            [PathBuf::from(&owl.file_path)]
        );
        assert_eq!(
            files_for_scene(&conn, story, 1).unwrap(),
            both,
            "other scenes are unaffected"
        );
        assert_eq!(
            list_off(&conn, story).unwrap(),
            [SceneCharacterOff {
                position: 0,
                character_id: hero.id
            }]
        );

        set_in_scene(&conn, story, 0, hero.id, true).unwrap();
        assert_eq!(files_for_scene(&conn, story, 0).unwrap(), both);
        assert!(
            set_in_scene(&conn, story, 0, 12345, false).is_err(),
            "unknown character"
        );
    }

    #[test]
    fn removing_a_character_returns_its_file_and_clears_its_switches() {
        let (conn, story, dir) = setup();
        let hero = add(&conn, &dir, story, "Tembo", "a.png", b"x").unwrap();
        set_in_scene(&conn, story, 2, hero.id, false).unwrap();

        assert_eq!(
            remove(&conn, story, hero.id).unwrap().as_deref(),
            Some(hero.file_path.as_str())
        );

        assert!(list(&conn, story).unwrap().is_empty());
        assert!(list_off(&conn, story).unwrap().is_empty());
        assert_eq!(remove(&conn, story, hero.id).unwrap(), None, "already gone");
    }

    #[test]
    fn deleting_the_story_deletes_its_characters() {
        let (conn, story, dir) = setup();
        let hero = add(&conn, &dir, story, "Tembo", "a.png", b"x").unwrap();
        set_in_scene(&conn, story, 0, hero.id, false).unwrap();
        conn.execute("DELETE FROM stories WHERE id = ?1", [story])
            .unwrap();
        assert!(list(&conn, story).unwrap().is_empty());
        assert!(list_off(&conn, story).unwrap().is_empty());
    }
}
