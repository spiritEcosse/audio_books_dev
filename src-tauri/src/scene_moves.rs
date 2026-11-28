//! Keeps a scene's pictures, narration, reference and character switches with the scene when the list changes.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::NewScene;
use crate::error::AppError;

/// Tables whose rows belong to the scene at their `position`.
const TABLES: [&str; 3] = ["scene_assets", "scene_references", "scene_character_off"];

/// Positions at or below this hold the rows of removed scenes, so that undoing the removal brings them back.
const FIRST_PARKED: i64 = -1000;

/// Added below every real or parked position while rows swap places, so that no two collide on the way.
const IN_TRANSIT: i64 = -1_000_000_000;

fn same(a: &str, b: &str) -> bool {
    !a.is_empty() && a == b
}

/// Gives each unmatched new scene the closest unused old scene that `fits`.
fn pair(
    matched: &mut [Option<i64>],
    used: &mut [i64],
    old: &[(i64, NewScene)],
    new: &[NewScene],
    fits: impl Fn(&NewScene, &NewScene) -> bool,
) {
    for (index, scene) in new.iter().enumerate() {
        if matched[index].is_some() {
            continue;
        }
        let found = old
            .iter()
            .filter(|(position, before)| !used.contains(position) && fits(before, scene))
            .min_by_key(|(position, _)| (position - index as i64).abs());
        if let Some((position, _)) = found {
            matched[index] = Some(*position);
            used[index] = *position;
        }
    }
}

/// For every new scene, the position (current or parked) of the scene it continues, if any.
fn match_scenes(
    current: &[(i64, NewScene)],
    parked: &[(i64, NewScene)],
    new: &[NewScene],
) -> Vec<Option<i64>> {
    let mut matched = vec![None; new.len()];
    // Indexed like `matched`; i64::MIN marks "nothing taken", a value no scene sits at.
    let mut used = vec![i64::MIN; new.len()];
    let unchanged = |a: &NewScene, b: &NewScene| a == b;
    let text = |a: &NewScene, b: &NewScene| same(&a.text, &b.text);
    let prompt = |a: &NewScene, b: &NewScene| same(&a.image_prompt, &b.image_prompt);
    pair(&mut matched, &mut used, current, new, unchanged);
    pair(&mut matched, &mut used, current, new, text);
    pair(&mut matched, &mut used, current, new, prompt);
    pair(&mut matched, &mut used, parked, new, unchanged);
    pair(&mut matched, &mut used, parked, new, text);
    // Rewritten in place: it keeps its media, which then shows as outdated.
    for index in 0..new.len() {
        let position = index as i64;
        if matched[index].is_none()
            && !used.contains(&position)
            && current.iter().any(|(p, _)| *p == position)
        {
            matched[index] = Some(position);
            used[index] = position;
        }
    }
    matched
}

fn scenes_from(
    conn: &Connection,
    sql: &str,
    story_id: i64,
) -> Result<Vec<(i64, NewScene)>, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([story_id], |row| {
        Ok((
            row.get(0)?,
            NewScene {
                text: row.get(1)?,
                image_prompt: row.get(2)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn has_rows(conn: &Connection, story_id: i64, position: i64) -> Result<bool, AppError> {
    for table in TABLES {
        let found: Option<i64> = conn
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE story_id = ?1 AND position = ?2 LIMIT 1"),
                params![story_id, position],
                |row| row.get(0),
            )
            .optional()?;
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Positions that hold rows although no scene is there (left by an older build).
fn orphans(
    conn: &Connection,
    story_id: i64,
    current: &[(i64, NewScene)],
) -> Result<Vec<i64>, AppError> {
    let mut found = Vec::new();
    for table in TABLES {
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT position FROM {table} WHERE story_id = ?1 AND position >= 0"
        ))?;
        for position in stmt.query_map([story_id], |row| row.get::<_, i64>(0))? {
            let position = position?;
            if !found.contains(&position) && current.iter().all(|(p, _)| *p != position) {
                found.push(position);
            }
        }
    }
    Ok(found)
}

/// Moves the rows of every scene to where the new list puts it. Call inside the transaction that replaces the
/// scenes, before the old ones are deleted.
pub fn follow(conn: &Connection, story_id: i64, new: &[NewScene]) -> Result<(), AppError> {
    let current = scenes_from(
        conn,
        "SELECT position, text, image_prompt FROM scenes WHERE story_id = ?1 ORDER BY position",
        story_id,
    )?;
    let parked = scenes_from(
        conn,
        "SELECT position, text, image_prompt FROM parked_scenes WHERE story_id = ?1 ORDER BY position DESC",
        story_id,
    )?;
    let matched = match_scenes(&current, &parked, new);

    let mut moves: HashMap<i64, i64> = matched
        .iter()
        .enumerate()
        .filter_map(|(index, from)| from.map(|from| (from, index as i64)))
        .collect();
    for (position, _) in &parked {
        if moves.contains_key(position) {
            conn.execute(
                "DELETE FROM parked_scenes WHERE story_id = ?1 AND position = ?2",
                params![story_id, position],
            )?;
        }
    }
    let mut next_slot = parked
        .iter()
        .map(|(position, _)| position - 1)
        .min()
        .unwrap_or(FIRST_PARKED);
    // Orphans have no known text, so they are put away and never matched.
    let unknown = NewScene {
        text: String::new(),
        image_prompt: String::new(),
    };
    let lost = orphans(conn, story_id, &current)?;
    let leaving = current
        .iter()
        .map(|(position, scene)| (*position, scene))
        .chain(lost.iter().map(|position| (*position, &unknown)));
    for (position, scene) in leaving {
        if !moves.contains_key(&position) && has_rows(conn, story_id, position)? {
            conn.execute(
                "INSERT INTO parked_scenes (story_id, position, text, image_prompt) VALUES (?1, ?2, ?3, ?4)",
                params![story_id, next_slot, scene.text, scene.image_prompt],
            )?;
            moves.insert(position, next_slot);
            next_slot -= 1;
        }
    }
    moves.retain(|from, to| from != to);

    for table in TABLES {
        for from in moves.keys() {
            conn.execute(
                &format!("UPDATE {table} SET position = ?3 + position WHERE story_id = ?1 AND position = ?2"),
                params![story_id, from, IN_TRANSIT],
            )?;
        }
        for (from, to) in &moves {
            // Whatever still sits at the target was left by a scene that no longer exists.
            conn.execute(
                &format!("UPDATE OR REPLACE {table} SET position = ?3 WHERE story_id = ?1 AND position = ?2"),
                params![story_id, IN_TRANSIT + from, to],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use crate::db::{self, NewScene};
    use crate::media::{self, KIND_AUDIO, KIND_IMAGE};
    use crate::{characters, collect, media_queue};

    fn scene(name: &str) -> NewScene {
        NewScene {
            text: format!("text {name}"),
            image_prompt: format!("prompt {name}"),
        }
    }

    fn set(conn: &mut Connection, story: i64, names: &[&str]) {
        let scenes: Vec<NewScene> = names.iter().map(|name| scene(name)).collect();
        db::replace_scenes(conn, story, &scenes).unwrap();
    }

    /// A story with the scenes a, b, c, each with a finished picture named after it.
    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap().id;
        set(&mut conn, story, &["a", "b", "c"]);
        for (position, name) in ["a", "b", "c"].iter().enumerate() {
            media::request_asset(&conn, story, position as i64, KIND_IMAGE).unwrap();
            conn.execute(
                "UPDATE scene_assets SET status = 'ready', file_path = ?2 WHERE position = ?1",
                rusqlite::params![position as i64, format!("/m/{name}.png")],
            )
            .unwrap();
        }
        (conn, story)
    }

    /// (position, picture, stale) of every listed picture.
    fn pictures(conn: &Connection, story: i64) -> Vec<(i64, String, bool)> {
        media::list_assets(conn, story)
            .unwrap()
            .into_iter()
            .filter(|a| a.kind == KIND_IMAGE)
            .map(|a| (a.position, a.file_path.unwrap_or_default(), a.stale))
            .collect()
    }

    fn fresh(position: i64, name: &str) -> (i64, String, bool) {
        (position, format!("/m/{name}.png"), false)
    }

    #[test]
    fn an_inserted_scene_shifts_the_media_with_their_scenes() {
        let (mut conn, story) = setup();
        set(&mut conn, story, &["a", "new", "b", "c"]);
        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "a"), fresh(2, "b"), fresh(3, "c")]
        );
    }

    #[test]
    fn reordered_scenes_keep_their_media() {
        let (mut conn, story) = setup();
        set(&mut conn, story, &["c", "a", "b"]);
        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "c"), fresh(1, "a"), fresh(2, "b")]
        );
    }

    #[test]
    fn a_removed_scene_hides_its_media_and_gets_them_back_when_it_returns() {
        let (mut conn, story) = setup();
        set(&mut conn, story, &["a", "c"]);
        assert_eq!(pictures(&conn, story), [fresh(0, "a"), fresh(1, "c")]);

        set(&mut conn, story, &["a", "b", "c"]);

        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "a"), fresh(1, "b"), fresh(2, "c")]
        );
    }

    #[test]
    fn removing_twice_keeps_both_removed_scenes_apart() {
        let (mut conn, story) = setup();
        set(&mut conn, story, &["a", "c"]);
        set(&mut conn, story, &["a"]);

        set(&mut conn, story, &["c", "a", "b"]);

        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "c"), fresh(1, "a"), fresh(2, "b")]
        );
    }

    #[test]
    fn a_rewritten_scene_keeps_its_media_as_outdated() {
        let (mut conn, story) = setup();
        set(&mut conn, story, &["a", "rewritten", "c"]);
        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "a"), (1, "/m/b.png".into(), true), fresh(2, "c")]
        );
    }

    #[test]
    fn a_scene_with_new_text_and_the_old_prompt_is_followed_when_it_moves() {
        let (mut conn, story) = setup();
        let mut edited = scene("b");
        edited.text = "other words".into();
        db::replace_scenes(&mut conn, story, &[edited, scene("a"), scene("c")]).unwrap();
        assert_eq!(
            pictures(&conn, story),
            [fresh(0, "b"), fresh(1, "a"), fresh(2, "c")]
        );
    }

    #[test]
    fn references_and_character_switches_move_too() {
        let (mut conn, story) = setup();
        media::set_reference(&conn, story, 1, "/r/owl.png").unwrap();
        conn.execute(
            "INSERT INTO story_characters (story_id, name, file_path) VALUES (?1, 'Owl', '/c/owl.png')",
            [story],
        )
        .unwrap();
        let owl = conn.last_insert_rowid();
        characters::set_in_scene(&conn, story, 1, owl, false).unwrap();

        set(&mut conn, story, &["new", "a", "b", "c"]);

        let references = media::list_references(&conn, story).unwrap();
        assert_eq!((references.len(), references[0].position), (1, 2));
        let off = characters::list_off(&conn, story).unwrap();
        assert_eq!((off.len(), off[0].position), (1, 2));
    }

    #[test]
    fn a_removed_scene_is_neither_queued_nor_collected() {
        let (mut conn, story) = setup();
        media::request_asset(&conn, story, 1, KIND_AUDIO).unwrap();
        media::request_asset(&conn, story, 2, KIND_AUDIO).unwrap();
        conn.execute(
            "UPDATE scene_assets SET started_at = CURRENT_TIMESTAMP, session_id = 's', flow_id = 'f'
             WHERE kind = 'audio' AND position = 2",
            [],
        )
        .unwrap();

        set(&mut conn, story, &["a"]);

        assert!(
            media_queue::queued_rows(&conn, story, KIND_AUDIO)
                .unwrap()
                .is_empty()
        );
        assert!(
            collect::pending_items(&conn, story, KIND_AUDIO)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rows_left_by_an_older_build_are_put_away_not_overwritten() {
        let (mut conn, story) = setup();
        // An older build kept the rows of a removed scene at its position.
        conn.execute("DELETE FROM scenes WHERE position = 2", [])
            .unwrap();

        set(&mut conn, story, &["new", "a", "b"]);

        assert_eq!(pictures(&conn, story), [fresh(1, "a"), fresh(2, "b")]);
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scene_assets WHERE file_path = '/m/c.png'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1);
    }

    #[test]
    fn a_running_generation_is_saved_to_the_scene_it_was_started_for() {
        let (mut conn, story) = setup();
        media::request_asset(&conn, story, 1, KIND_AUDIO).unwrap();
        media_queue::mark_started(&conn, story, KIND_AUDIO, &[1]).unwrap();
        let tracked = media_queue::Tracked::new(&conn, story, KIND_AUDIO, &[1]).unwrap();

        set(&mut conn, story, &["new", "a", "b", "c"]);

        assert_eq!(tracked.now(&conn, 1).unwrap(), Some(2));
    }

    #[test]
    fn a_generation_is_still_found_after_its_scene_was_removed() {
        let (mut conn, story) = setup();
        media::request_asset(&conn, story, 1, KIND_AUDIO).unwrap();
        media_queue::mark_started(&conn, story, KIND_AUDIO, &[1]).unwrap();
        let tracked = media_queue::Tracked::new(&conn, story, KIND_AUDIO, &[1]).unwrap();

        set(&mut conn, story, &["a", "c"]);
        let now = tracked.now(&conn, 1).unwrap().unwrap();
        let saved = media_queue::Saved {
            path: "/m/b.mp3".into(),
            credits: 1.0,
        };
        media_queue::apply_results(
            &conn,
            story,
            KIND_AUDIO,
            &[media_queue::ItemResult {
                position: now,
                saved: Ok(saved),
            }],
        )
        .unwrap();
        set(&mut conn, story, &["a", "b", "c"]);

        let audio: Vec<_> = media::list_assets(&conn, story)
            .unwrap()
            .into_iter()
            .filter(|a| a.kind == KIND_AUDIO)
            .map(|a| (a.position, a.status))
            .collect();
        assert_eq!(audio, [(1, "ready".to_owned())]);
    }
}
