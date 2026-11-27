//! One story's upload to one channel from start to finish, kept in the `youtube_uploads` table so that it survives errors and restarts:
//! an unfinished session is resumed, a finished video is not sent again, failed subtitles can be retried alone.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::form::Form;
use super::upload::{self, Api};
use crate::error::AppError;
use crate::upscale;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub resolution: String,
    /// What the session was opened for: the file and the metadata.
    stamp: String,
    session_url: Option<String>,
    pub video_id: Option<String>,
    /// The video of a previous upload, while a new one is under way.
    earlier_video_id: Option<String>,
    pub privacy: Option<String>,
    pub publish_at: Option<String>,
    /// The subtitles as they were when this video was uploaded: a later export does not change them.
    subtitles: Option<String>,
    pub captions_done: bool,
    pub captions_error: Option<String>,
}

/// What the UI shows about the last upload.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct View {
    pub resolution: String,
    pub video_id: Option<String>,
    /// As YouTube returned it, which may differ from what was asked.
    pub privacy: Option<String>,
    pub publish_at: Option<String>,
    /// "none", "uploaded" or "pending" (which a retry sends).
    pub subtitles: &'static str,
    pub captions_error: Option<String>,
    /// An unfinished upload that the next attempt continues.
    pub unfinished: bool,
    /// The channel has a video of this story already, so another upload needs a confirmation.
    pub already_uploaded: bool,
}

impl Record {
    fn on_youtube(&self) -> bool {
        self.video_id.is_some() || self.earlier_video_id.is_some()
    }

    pub fn view(&self) -> View {
        View {
            resolution: self.resolution.clone(),
            video_id: self.video_id.clone(),
            privacy: self.privacy.clone(),
            publish_at: self.publish_at.clone(),
            subtitles: match &self.subtitles {
                None => "none",
                Some(_) if self.captions_done => "uploaded",
                Some(_) => "pending",
            },
            captions_error: self.captions_error.clone(),
            // A session of an older 1080p file cannot continue: the upload now sends the 4K video.
            unfinished: self.video_id.is_none()
                && self.session_url.is_some()
                && self.resolution == upscale::LABEL,
            already_uploaded: self.on_youtube(),
        }
    }
}

fn load_column(conn: &Connection, story_id: i64, sql: &str) -> Result<Option<String>, AppError> {
    Ok(conn
        .query_row(sql, [story_id], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

fn encode(value: &impl Serialize) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|e| AppError::new(format!("cannot save: {e}")))
}

pub fn load_form(conn: &Connection, story_id: i64) -> Result<Option<Form>, AppError> {
    let json = load_column(
        conn,
        story_id,
        "SELECT form FROM youtube WHERE story_id = ?1",
    )?;
    Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
}

pub fn save_form(conn: &Connection, story_id: i64, form: &Form) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO youtube (story_id, form) VALUES (?1, ?2)
         ON CONFLICT(story_id) DO UPDATE SET form = excluded.form",
        params![story_id, encode(form)?],
    )?;
    Ok(())
}

fn decode(json: &str) -> Result<Record, AppError> {
    serde_json::from_str(json)
        .map_err(|e| AppError::new(format!("the saved upload is unreadable: {e}")))
}

pub fn load_record(
    conn: &Connection,
    story_id: i64,
    channel_id: &str,
) -> Result<Option<Record>, AppError> {
    conn.query_row(
        "SELECT upload FROM youtube_uploads WHERE story_id = ?1 AND channel_id = ?2",
        params![story_id, channel_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| decode(&json))
    .transpose()
}

/// The story's uploads with the channel of each.
pub fn load_records(conn: &Connection, story_id: i64) -> Result<Vec<(String, Record)>, AppError> {
    let mut statement = conn.prepare(
        "SELECT channel_id, upload FROM youtube_uploads WHERE story_id = ?1 ORDER BY rowid",
    )?;
    let rows = statement.query_map([story_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (channel_id, json) = row?;
        Ok((channel_id, decode(&json)?))
    })
    .collect()
}

pub fn save_record(
    conn: &Connection,
    story_id: i64,
    channel_id: &str,
    record: &Record,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO youtube_uploads (story_id, channel_id, upload) VALUES (?1, ?2, ?3)
         ON CONFLICT(story_id, channel_id) DO UPDATE SET upload = excluded.upload",
        params![story_id, channel_id, encode(record)?],
    )?;
    Ok(())
}

/// Whether the story has an upload from an older build that no channel has been given yet.
pub fn has_unadopted(conn: &Connection, story_id: i64) -> Result<bool, AppError> {
    Ok(load_column(
        conn,
        story_id,
        "SELECT upload FROM youtube WHERE story_id = ?1",
    )?
    .is_some())
}

/// Gives the uploads saved before there were channels to `channel_id`. Unless that channel is `verified` as the one
/// that made them, an unfinished session is dropped: it would continue on the channel that opened it.
pub fn adopt_records(conn: &Connection, channel_id: &str, verified: bool) -> Result<(), AppError> {
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO youtube_uploads (story_id, channel_id, upload)
         SELECT story_id, ?1, CASE WHEN ?2 THEN upload ELSE json_remove(upload, '$.session_url') END
         FROM youtube WHERE upload IS NOT NULL",
        params![channel_id, verified],
    )?;
    transaction.execute(
        "UPDATE youtube SET upload = NULL WHERE upload IS NOT NULL",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

fn stamp(video: &Path, metadata: &Value) -> Result<(String, u64), AppError> {
    let meta = std::fs::metadata(video)
        .map_err(|e| AppError::new(format!("cannot read {}: {e}", video.display())))?;
    let mut hasher = DefaultHasher::new();
    (
        video,
        meta.len(),
        meta.modified().ok(),
        metadata.to_string(),
    )
        .hash(&mut hasher);
    Ok((format!("{:016x}", hasher.finish()), meta.len()))
}

pub struct Request<'a> {
    pub video: &'a Path,
    pub metadata: Value,
    pub subtitles: Option<String>,
    /// The user confirmed uploading a story that is already on YouTube.
    pub again: bool,
}

/// Sends the subtitles of `record`. A failure is recorded, not returned: the video is already up.
fn send_captions(api: &Api, record: Record) -> Record {
    let (Some(video_id), Some(srt)) = (&record.video_id, &record.subtitles) else {
        return record;
    };
    let outcome = upload::captions(api, video_id, srt);
    Record {
        captions_done: outcome.is_ok(),
        captions_error: outcome.err().map(|error| error.to_string()),
        ..record
    }
}

/// Uploads the video, then the subtitles. `save` is called whenever there is something worth keeping.
pub fn run(
    api: &Api,
    request: Request,
    previous: Option<Record>,
    cancel: &AtomicBool,
    save: &mut dyn FnMut(&Record) -> Result<(), AppError>,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<Record, AppError> {
    let previous = previous.unwrap_or_default();
    if previous.video_id.is_some() && !request.again {
        return Err(AppError::new("this story is already on this channel"));
    }
    let (stamp, size) = stamp(request.video, &request.metadata)?;
    let mut record = Record {
        resolution: upscale::LABEL.to_owned(),
        session_url: previous
            .session_url
            .filter(|_| previous.video_id.is_none() && previous.stamp == stamp),
        earlier_video_id: previous.video_id.or(previous.earlier_video_id),
        subtitles: request.subtitles,
        stamp,
        ..Record::default()
    };
    // A saved session may have expired; a new one is then opened, once.
    for _ in 0..2 {
        let session = match &record.session_url {
            Some(url) => url.clone(),
            None => {
                let url = upload::start(api, &request.metadata, size)?;
                record.session_url = Some(url.clone());
                url
            }
        };
        save(&record)?;
        match upload::send(api, &session, request.video, cancel, on_progress)? {
            Some(uploaded) => {
                let uploaded = Record {
                    session_url: None,
                    video_id: Some(uploaded.video_id),
                    privacy: uploaded.privacy,
                    publish_at: uploaded.publish_at,
                    ..record
                };
                save(&uploaded)?;
                let done = send_captions(api, uploaded);
                save(&done)?;
                return Ok(done);
            }
            None => record.session_url = None,
        }
    }
    Err(AppError::new("YouTube dropped the upload session twice"))
}

/// Retries the subtitles of an uploaded video without touching the video.
pub fn retry_captions(api: &Api, record: Record) -> Result<Record, AppError> {
    if record.video_id.is_none() || record.subtitles.is_none() || record.captions_done {
        return Err(AppError::new(
            "there are no subtitles waiting to be uploaded",
        ));
    }
    Ok(send_captions(api, record))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use serde_json::json;

    use super::super::fake::Server;
    use super::*;
    use crate::db;

    const VIDEO: &str = r#"{"id":"vid","status":{"privacyStatus":"private"}}"#;
    const SESSION: &str = "Location: {url}/session";

    fn api(server: &Server) -> Api {
        Api {
            base: server.url.clone(),
            token: Box::new(|| Ok("tok".into())),
            backoff: Duration::from_millis(1),
        }
    }

    fn video(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("audiobooks-job-{name}-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        path
    }

    fn go(
        server: &Server,
        file: &Path,
        previous: Option<Record>,
        again: bool,
        cancel: bool,
    ) -> (Result<Record, AppError>, Option<Record>) {
        let request = Request {
            video: file,
            metadata: json!({"snippet": {"title": "T"}}),
            subtitles: Some("srt".into()),
            again,
        };
        let flag = AtomicBool::new(false);
        let mut saved = None;
        let result = run(
            &api(server),
            request,
            previous,
            &flag,
            &mut |record| {
                saved = Some(record.clone());
                Ok(())
            },
            &mut |_, _| flag.store(cancel, Ordering::SeqCst),
        );
        (result, saved)
    }

    #[test]
    fn an_unfinished_upload_of_an_older_1080p_video_is_not_offered_for_resuming() {
        let record = |resolution: &str| Record {
            resolution: resolution.into(),
            session_url: Some("u".into()),
            ..Record::default()
        };
        assert!(record("4k").view().unfinished);
        assert!(!record("1080p").view().unfinished);
    }

    #[test]
    fn a_failed_caption_upload_keeps_the_video_and_can_be_retried_alone() {
        let file = video("captions");
        let server = Server::start(&[
            ("200 OK", SESSION, ""),
            ("308 Resume Incomplete", "", ""),
            ("200 OK", "", VIDEO),
            (
                "400 Bad Request",
                "",
                r#"{"error":{"message":"no captions today"}}"#,
            ),
        ]);
        let record = go(&server, &file, None, false, false).0.unwrap();
        let view = record.view();
        assert_eq!(view.video_id.as_deref(), Some("vid"));
        assert_eq!(view.privacy.as_deref(), Some("private"));
        assert_eq!(view.subtitles, "pending");
        assert!(view.captions_error.unwrap().contains("no captions today"));

        let server = Server::start(&[("200 OK", "", "{}")]);
        let retried = retry_captions(&api(&server), record).unwrap();
        assert_eq!(retried.view().subtitles, "uploaded");
        assert_eq!(server.requests().len(), 1);
        assert!(retry_captions(&api(&server), retried).is_err());
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_cancelled_upload_keeps_its_session_and_the_next_one_continues_it() {
        let file = video("resume");
        let server = Server::start(&[("200 OK", SESSION, ""), ("308 Resume Incomplete", "", "")]);
        let (result, saved) = go(&server, &file, None, false, true);
        assert_eq!(result.unwrap_err().to_string(), upload::CANCELLED_MESSAGE);
        let saved = saved.unwrap();
        assert!(saved.view().unfinished);

        // No second `POST`: the saved session (on the first server, repointed here) is asked where it is.
        let server = Server::start(&[("200 OK", "", VIDEO), ("200 OK", "", "{}")]);
        let moved = Record {
            session_url: Some(format!("{}/session", server.url)),
            ..saved
        };
        let record = go(&server, &file, Some(moved), false, false).0.unwrap();
        assert_eq!(record.view().subtitles, "uploaded");
        assert!(server.requests()[0].line.starts_with("PUT /session"));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn an_expired_session_is_replaced_by_a_new_one() {
        let file = video("expired");
        let server = Server::start(&[
            ("200 OK", SESSION, ""),
            ("404 Not Found", "", ""),
            ("200 OK", SESSION, ""),
            ("200 OK", "", VIDEO),
            ("200 OK", "", "{}"),
        ]);
        assert!(go(&server, &file, None, false, false).0.is_ok());
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_second_upload_needs_a_confirmation_even_after_it_failed_midway() {
        let file = video("again");
        let uploaded = Record {
            video_id: Some("old".into()),
            ..Record::default()
        };
        let server = Server::start(&[]);
        let error = go(&server, &file, Some(uploaded.clone()), false, false)
            .0
            .unwrap_err();
        assert!(error.to_string().contains("already on this channel"));

        let server = Server::start(&[("200 OK", SESSION, ""), ("308 Resume Incomplete", "", "")]);
        let saved = go(&server, &file, Some(uploaded), true, true).1.unwrap();
        assert!(saved.view().already_uploaded && saved.view().unfinished);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn uploads_are_stored_per_story_and_channel_and_go_with_the_story() {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "S", "").unwrap().id;
        assert_eq!(load_form(&conn, story).unwrap(), None);
        assert_eq!(load_record(&conn, story, "a").unwrap(), None);
        let form = Form {
            title: "T".into(),
            ..Form::default()
        };
        let record = Record {
            resolution: "4k".into(),
            ..Record::default()
        };
        save_record(&conn, story, "a", &record).unwrap();
        save_form(&conn, story, &form).unwrap();
        assert_eq!(load_form(&conn, story).unwrap(), Some(form));
        assert_eq!(
            load_record(&conn, story, "a").unwrap(),
            Some(record.clone())
        );
        assert_eq!(load_record(&conn, story, "b").unwrap(), None);
        save_record(&conn, story, "b", &Record::default()).unwrap();
        assert_eq!(
            load_records(&conn, story).unwrap(),
            [
                ("a".to_owned(), record),
                ("b".to_owned(), Record::default())
            ]
        );
        db::delete_story(&conn, story).unwrap();
        assert!(load_records(&conn, story).unwrap().is_empty());
    }
}
