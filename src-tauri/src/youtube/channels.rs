//! The connected channels: their names and pictures in SQLite, their tokens only in the Keychain.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::job;
use super::keychain::{self, LEGACY_ACCOUNT, Tokens};
use super::upload::{self, Api};
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
}

/// The channel the access token of `api` was granted for.
pub fn mine(api: &Api) -> Result<Channel, AppError> {
    #[derive(Deserialize)]
    struct Picture {
        url: String,
    }
    #[derive(Deserialize)]
    struct Thumbnails {
        default: Option<Picture>,
    }
    #[derive(Deserialize)]
    struct Snippet {
        title: String,
        thumbnails: Option<Thumbnails>,
    }
    #[derive(Deserialize)]
    struct Item {
        id: String,
        snippet: Snippet,
    }
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        items: Vec<Item>,
    }
    let text = upload::get(api, "/youtube/v3/channels?part=snippet&mine=true")?;
    let reply: Reply = serde_json::from_str(&text)
        .map_err(|e| AppError::new(format!("unexpected answer from YouTube: {e}")))?;
    let item = reply.items.into_iter().next().ok_or_else(|| {
        AppError::new("this account has no YouTube channel: create one or choose another account")
    })?;
    Ok(Channel {
        id: item.id,
        title: item.snippet.title,
        thumbnail: item
            .snippet
            .thumbnails
            .and_then(|all| all.default)
            .map(|picture| picture.url),
    })
}

/// In the order they were first connected.
pub fn list(conn: &Connection) -> Result<Vec<Channel>, AppError> {
    let mut statement =
        conn.prepare("SELECT id, title, thumbnail FROM youtube_channels ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok(Channel {
            id: row.get(0)?,
            title: row.get(1)?,
            thumbnail: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn store(
    conn: &Connection,
    tokens: &dyn Tokens,
    channel: &Channel,
    token: &str,
) -> Result<(), AppError> {
    tokens.save(&keychain::account(&channel.id), token)?;
    conn.execute(
        "INSERT INTO youtube_channels (id, title, thumbnail) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, thumbnail = excluded.thumbnail",
        params![channel.id, channel.title, channel.thumbnail],
    )?;
    Ok(())
}

/// Stores a channel that has just signed in; an already connected one gets the new token.
pub fn connect(
    conn: &Connection,
    tokens: &dyn Tokens,
    channel: &Channel,
    token: &str,
) -> Result<(), AppError> {
    store(conn, tokens, channel, token)?;
    // While the old single token is still there, the old upload records belong to its channel.
    if tokens.load(LEGACY_ACCOUNT)?.is_none() {
        job::adopt_records(conn, &channel.id, false)?;
    }
    Ok(())
}

/// Moves the single token of an older build, and its upload records, to the channel it signs in to. `channel` is
/// `None` when that sign-in has expired or is given up: the records then go to the first channel, now or when one
/// is connected.
pub fn adopt_legacy(
    conn: &Connection,
    tokens: &dyn Tokens,
    channel: Option<&Channel>,
) -> Result<(), AppError> {
    let Some(token) = tokens.load(LEGACY_ACCOUNT)? else {
        return Ok(());
    };
    if let Some(channel) = channel {
        // A sign-in made meanwhile is newer than the old token.
        let newer = tokens.load(&keychain::account(&channel.id))?;
        store(conn, tokens, channel, newer.as_deref().unwrap_or(&token))?;
        job::adopt_records(conn, &channel.id, true)?;
    } else if let Some(first) = list(conn)?.first() {
        job::adopt_records(conn, &first.id, false)?;
    }
    tokens.delete(LEGACY_ACCOUNT)
}

/// Forgets the channel and returns its token, so that it can be revoked. The upload records stay: connecting the
/// channel again brings them back.
pub fn remove(
    conn: &Connection,
    tokens: &dyn Tokens,
    channel_id: &str,
) -> Result<Option<String>, AppError> {
    let account = keychain::account(channel_id);
    let token = tokens.load(&account)?;
    tokens.delete(&account)?;
    conn.execute("DELETE FROM youtube_channels WHERE id = ?1", [channel_id])?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::fake::Server;
    use super::super::keychain::tests::Memory;
    use super::*;
    use crate::db;

    fn channel(id: &str, title: &str) -> Channel {
        Channel {
            id: id.into(),
            title: title.into(),
            thumbnail: None,
        }
    }

    fn api(server: &Server) -> Api {
        Api {
            base: server.url.clone(),
            token: Box::new(|| Ok("tok".into())),
            backoff: Duration::from_millis(1),
        }
    }

    /// A database with one story uploaded by a build that knew a single account.
    fn old_database() -> (Connection, i64) {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "S", "").unwrap().id;
        conn.execute(
            "INSERT INTO youtube (story_id, upload) VALUES (?1, ?2)",
            params![story, r#"{"resolution":"4k","video_id":"old"}"#],
        )
        .unwrap();
        (conn, story)
    }

    fn uploaded(conn: &Connection, story: i64, channel_id: &str) -> Option<String> {
        job::load_record(conn, story, channel_id)
            .unwrap()
            .and_then(|record| record.video_id)
    }

    #[test]
    fn mine_reads_the_name_and_picture_of_the_signed_in_channel() {
        let server = Server::start(&[(
            "200 OK",
            "",
            r#"{"items":[{"id":"UC1","snippet":{"title":"Tales","thumbnails":{"default":{"url":"https://p"}}}}]}"#,
        )]);
        let found = mine(&api(&server)).unwrap();
        assert_eq!(
            found,
            Channel {
                thumbnail: Some("https://p".into()),
                ..channel("UC1", "Tales")
            }
        );
        let request = &server.requests()[0];
        assert_eq!(
            request.line,
            "GET /youtube/v3/channels?part=snippet&mine=true"
        );
        assert_eq!(request.header("authorization"), "bearer tok");

        let server = Server::start(&[("200 OK", "", "{}")]);
        let error = mine(&api(&server)).unwrap_err().to_string();
        assert!(error.contains("no YouTube channel"), "{error}");
    }

    #[test]
    fn signing_in_again_refreshes_the_channel_instead_of_adding_it_twice() {
        let (conn, _) = old_database();
        let tokens = Memory::default();
        connect(&conn, &tokens, &channel("UC1", "Old name"), "one").unwrap();
        connect(&conn, &tokens, &channel("UC2", "Second"), "two").unwrap();
        connect(&conn, &tokens, &channel("UC1", "New name"), "three").unwrap();
        assert_eq!(
            list(&conn).unwrap(),
            [channel("UC1", "New name"), channel("UC2", "Second")]
        );
        assert_eq!(
            tokens.load(&keychain::account("UC1")).unwrap().as_deref(),
            Some("three")
        );
    }

    #[test]
    fn the_old_token_and_uploads_move_to_its_channel_even_if_another_was_connected_first() {
        let (conn, story) = old_database();
        let tokens = Memory::default();
        tokens.save(LEGACY_ACCOUNT, "old-token").unwrap();
        connect(&conn, &tokens, &channel("UC2", "Other"), "two").unwrap();
        assert_eq!(uploaded(&conn, story, "UC2"), None);

        adopt_legacy(&conn, &tokens, Some(&channel("UC1", "Mine"))).unwrap();
        assert_eq!(uploaded(&conn, story, "UC1").as_deref(), Some("old"));
        assert_eq!(
            tokens.load(&keychain::account("UC1")).unwrap().as_deref(),
            Some("old-token")
        );
        assert_eq!(tokens.load(LEGACY_ACCOUNT).unwrap(), None);
        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn after_an_expired_old_sign_in_the_first_channel_gets_the_old_uploads() {
        let (conn, story) = old_database();
        let tokens = Memory::default();
        tokens.save(LEGACY_ACCOUNT, "expired").unwrap();
        adopt_legacy(&conn, &tokens, None).unwrap();
        assert_eq!(tokens.load(LEGACY_ACCOUNT).unwrap(), None);
        assert!(list(&conn).unwrap().is_empty());

        connect(&conn, &tokens, &channel("UC1", "Mine"), "one").unwrap();
        connect(&conn, &tokens, &channel("UC2", "Second"), "two").unwrap();
        assert_eq!(uploaded(&conn, story, "UC1").as_deref(), Some("old"));
        assert_eq!(uploaded(&conn, story, "UC2"), None);
    }

    #[test]
    fn an_old_sign_in_that_expires_after_a_channel_was_added_leaves_its_uploads_there() {
        let (conn, story) = old_database();
        let tokens = Memory::default();
        tokens.save(LEGACY_ACCOUNT, "expired").unwrap();
        connect(&conn, &tokens, &channel("UC1", "Mine"), "one").unwrap();
        assert!(job::has_unadopted(&conn, story).unwrap());

        adopt_legacy(&conn, &tokens, None).unwrap();
        assert!(!job::has_unadopted(&conn, story).unwrap());
        assert_eq!(uploaded(&conn, story, "UC1").as_deref(), Some("old"));
    }

    #[test]
    fn an_unfinished_session_is_kept_only_for_the_channel_known_to_have_opened_it() {
        let unfinished = |verified: bool| {
            let (conn, story) = old_database();
            conn.execute(
                "UPDATE youtube SET upload = ?1",
                [r#"{"resolution":"4k","session_url":"https://session"}"#],
            )
            .unwrap();
            let tokens = Memory::default();
            if verified {
                tokens.save(LEGACY_ACCOUNT, "old-token").unwrap();
                adopt_legacy(&conn, &tokens, Some(&channel("UC1", "Mine"))).unwrap();
            } else {
                connect(&conn, &tokens, &channel("UC1", "Mine"), "one").unwrap();
            }
            let record = job::load_record(&conn, story, "UC1").unwrap().unwrap();
            record.view().unfinished
        };
        assert!(unfinished(true));
        assert!(!unfinished(false));
    }

    #[test]
    fn disconnecting_removes_one_channel_and_its_token_but_keeps_its_uploads() {
        let (conn, story) = old_database();
        let tokens = Memory::default();
        connect(&conn, &tokens, &channel("UC1", "Mine"), "one").unwrap();
        connect(&conn, &tokens, &channel("UC2", "Second"), "two").unwrap();
        assert_eq!(
            remove(&conn, &tokens, "UC1").unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(list(&conn).unwrap(), [channel("UC2", "Second")]);
        assert_eq!(tokens.load(&keychain::account("UC1")).unwrap(), None);
        assert!(tokens.load(&keychain::account("UC2")).unwrap().is_some());
        assert_eq!(uploaded(&conn, story, "UC1").as_deref(), Some("old"));
    }
}
