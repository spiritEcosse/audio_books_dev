//! State-changing operations that keep a revision trail, so every replacement can be undone.

use rusqlite::Connection;

use crate::db::{self, NewScene};
use crate::error::AppError;

pub const KIND_STORY_TEXT: &str = "story_text";
pub const KIND_SCENES: &str = "scenes";

fn snapshot_scenes(conn: &Connection, story_id: i64) -> Result<(), AppError> {
    let scenes: Vec<NewScene> = db::list_scenes(conn, story_id)?
        .into_iter()
        .map(|s| NewScene {
            text: s.text,
            image_prompt: s.image_prompt,
        })
        .collect();
    if scenes.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_string(&scenes)
        .map_err(|e| AppError::new(format!("cannot snapshot scenes: {e}")))?;
    db::add_revision(conn, story_id, KIND_SCENES, &payload)
}

fn set_story_text(conn: &Connection, story_id: i64, text: &str) -> Result<(), AppError> {
    let current = db::get_story(conn, story_id)?;
    db::add_revision(conn, story_id, KIND_STORY_TEXT, &current.text)?;
    db::update_story_text(conn, story_id, text)
}

/// Replaces all scenes, saving the previous ones as a revision first.
pub fn apply_scenes(
    conn: &mut Connection,
    story_id: i64,
    scenes: &[NewScene],
) -> Result<(), AppError> {
    snapshot_scenes(conn, story_id)?;
    db::replace_scenes(conn, story_id, scenes)
}

/// Saves the proposed story text. Returns the story id.
pub fn accept_proposal(conn: &Connection, message_id: i64) -> Result<i64, AppError> {
    let (story_id, proposal) = db::get_proposal(conn, message_id)?;
    if proposal.state != "pending" {
        return Err(AppError::new("this proposal was already answered"));
    }
    set_story_text(conn, story_id, &proposal.text)?;
    db::set_proposal_state(conn, message_id, "accepted")?;
    db::add_note(
        conn,
        story_id,
        "The user ACCEPTED your proposed story text; it is now the story's saved text.",
    )?;
    Ok(story_id)
}

pub fn reject_proposal(conn: &Connection, message_id: i64) -> Result<i64, AppError> {
    let (story_id, proposal) = db::get_proposal(conn, message_id)?;
    if proposal.state != "pending" {
        return Err(AppError::new("this proposal was already answered"));
    }
    db::set_proposal_state(conn, message_id, "rejected")?;
    db::add_note(
        conn,
        story_id,
        "The user REJECTED your proposed story text; the saved story text is unchanged.",
    )?;
    Ok(story_id)
}

/// Restores an older version. The current state is snapshotted first, so a restore can be
/// undone too. Returns the story id.
pub fn restore_revision(conn: &mut Connection, revision_id: i64) -> Result<i64, AppError> {
    let revision = db::get_revision(conn, revision_id)?;
    let story_id = revision.story_id;
    match revision.kind.as_str() {
        KIND_STORY_TEXT => {
            set_story_text(conn, story_id, &revision.payload)?;
            db::add_note(
                conn,
                story_id,
                &format!(
                    "The user restored an earlier version of the story text. The saved story text is now:\n{}",
                    revision.payload
                ),
            )?;
        }
        KIND_SCENES => {
            let scenes: Vec<NewScene> = serde_json::from_str(&revision.payload)
                .map_err(|e| AppError::new(format!("corrupt scenes revision: {e}")))?;
            apply_scenes(conn, story_id, &scenes)?;
            db::add_note(
                conn,
                story_id,
                &format!(
                    "The user restored an earlier version of the scenes. The current scenes are now:\n{}",
                    revision.payload
                ),
            )?;
        }
        other => return Err(AppError::new(format!("unknown revision kind: {other}"))),
    }
    Ok(story_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, i64) {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "T", "original").unwrap();
        (conn, story.id)
    }

    fn scene(text: &str) -> NewScene {
        NewScene {
            text: text.into(),
            image_prompt: "p".into(),
        }
    }

    fn proposal_message(conn: &Connection, story_id: i64, text: &str) -> i64 {
        let msg = db::add_message(conn, story_id, "assistant", "here", false).unwrap();
        db::add_proposal(conn, msg.id, text).unwrap();
        msg.id
    }

    #[test]
    fn accepting_replaces_text_and_keeps_old_as_revision() {
        let (conn, id) = setup();
        let msg = proposal_message(&conn, id, "rewritten");

        accept_proposal(&conn, msg).unwrap();

        assert_eq!(db::get_story(&conn, id).unwrap().text, "rewritten");
        let revisions = db::list_revisions(&conn, id).unwrap();
        assert_eq!(
            (revisions[0].kind.as_str(), revisions[0].payload.as_str()),
            (KIND_STORY_TEXT, "original")
        );
        assert_eq!(db::list_notes(&conn, id).unwrap().len(), 1);
    }

    #[test]
    fn rejecting_leaves_text_untouched() {
        let (conn, id) = setup();
        let msg = proposal_message(&conn, id, "rewritten");

        reject_proposal(&conn, msg).unwrap();

        assert_eq!(db::get_story(&conn, id).unwrap().text, "original");
        assert!(db::list_revisions(&conn, id).unwrap().is_empty());
    }

    #[test]
    fn a_proposal_can_only_be_answered_once() {
        let (conn, id) = setup();
        let msg = proposal_message(&conn, id, "rewritten");
        accept_proposal(&conn, msg).unwrap();

        assert!(accept_proposal(&conn, msg).is_err());
        assert!(reject_proposal(&conn, msg).is_err());
    }

    #[test]
    fn applying_scenes_snapshots_the_previous_ones() {
        let (mut conn, id) = setup();
        apply_scenes(&mut conn, id, &[scene("first")]).unwrap();
        assert!(
            db::list_revisions(&conn, id).unwrap().is_empty(),
            "nothing to snapshot yet"
        );

        apply_scenes(&mut conn, id, &[scene("second")]).unwrap();

        let revisions = db::list_revisions(&conn, id).unwrap();
        assert_eq!(revisions.len(), 1);
        assert!(revisions[0].payload.contains("first"));
    }

    #[test]
    fn restoring_scenes_brings_them_back_and_is_itself_undoable() {
        let (mut conn, id) = setup();
        apply_scenes(&mut conn, id, &[scene("first")]).unwrap();
        apply_scenes(&mut conn, id, &[scene("second")]).unwrap();
        let old = db::list_revisions(&conn, id).unwrap()[0].id;

        restore_revision(&mut conn, old).unwrap();

        let texts: Vec<_> = db::list_scenes(&conn, id)
            .unwrap()
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(texts, ["first"]);
        assert!(
            db::list_revisions(&conn, id).unwrap()[0]
                .payload
                .contains("second")
        );
    }

    #[test]
    fn restoring_story_text_swaps_it_back() {
        let (mut conn, id) = setup();
        let msg = proposal_message(&conn, id, "rewritten");
        accept_proposal(&conn, msg).unwrap();
        let old = db::list_revisions(&conn, id).unwrap()[0].id;

        restore_revision(&mut conn, old).unwrap();

        assert_eq!(db::get_story(&conn, id).unwrap().text, "original");
    }
}
