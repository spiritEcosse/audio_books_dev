//! `videos.insert` with the resumable protocol, and `captions.insert`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::http::Response;

use crate::error::AppError;

/// A multiple of 256 KiB, as the protocol requires.
const CHUNK_BYTES: u64 = 8 * 1024 * 1024;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Failures in a row before giving up; the session stays, so the user can resume.
const ATTEMPTS: u32 = 5;
const BOUNDARY: &str = "audiobooks-caption-boundary";
pub const CANCELLED_MESSAGE: &str = "upload cancelled";

pub struct Api {
    pub base: String,
    /// Asked before every request: a long upload outlives one access token.
    pub token: Box<dyn Fn() -> Result<String, AppError>>,
    /// The first pause after a failure; it doubles each time.
    pub backoff: Duration,
}

impl Api {
    fn bearer(&self) -> Result<String, AppError> {
        Ok(format!("Bearer {}", (self.token)()?))
    }
}

/// What YouTube says it did with the video, which may differ from what was asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Uploaded {
    pub video_id: String,
    pub privacy: Option<String>,
    pub publish_at: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        // 308 means "send more" here, not a redirect.
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(CHUNK_TIMEOUT))
        .build()
        .into()
}

fn unreachable(error: ureq::Error) -> AppError {
    AppError::new(format!("cannot reach YouTube: {error}"))
}

fn header(response: &Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// A refusal from the API in words the user can act on.
fn refusal(mut response: Response<ureq::Body>) -> AppError {
    let status = response.status().as_u16();
    let body: Value = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let reason = body["error"]["errors"][0]["reason"].as_str().unwrap_or("");
    let message = body["error"]["message"].as_str().unwrap_or("");
    AppError::new(match (status, reason) {
        (_, "quotaExceeded") => {
            "the YouTube API quota for today is used up: try again tomorrow".to_owned()
        }
        (_, "uploadLimitExceeded") => {
            "the channel has reached its upload limit: try again later".to_owned()
        }
        (401, _) => "YouTube rejected the sign-in: connect the account again".to_owned(),
        _ => format!("YouTube refused (HTTP {status}) {reason} {message}"),
    })
}

fn video(mut response: Response<ureq::Body>) -> Result<Uploaded, AppError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Status {
        privacy_status: Option<String>,
        publish_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct Video {
        id: String,
        status: Option<Status>,
    }
    let text = response.body_mut().read_to_string().map_err(unreachable)?;
    let video: Video = serde_json::from_str(&text)
        .map_err(|e| AppError::new(format!("unexpected answer from YouTube: {e}")))?;
    let (privacy, publish_at) = video
        .status
        .map(|status| (status.privacy_status, status.publish_at))
        .unwrap_or_default();
    Ok(Uploaded {
        video_id: video.id,
        privacy,
        publish_at,
    })
}

/// Opens an upload session and returns its address.
pub fn start(api: &Api, metadata: &Value, size: u64) -> Result<String, AppError> {
    let url = format!(
        "{}/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status",
        api.base
    );
    let response = agent()
        .post(&url)
        .header("Authorization", &api.bearer()?)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("X-Upload-Content-Type", "video/mp4")
        .header("X-Upload-Content-Length", &size.to_string())
        .send(metadata.to_string())
        .map_err(unreachable)?;
    if !response.status().is_success() {
        return Err(refusal(response));
    }
    header(&response, "location").ok_or_else(|| AppError::new("YouTube gave no upload address"))
}

enum Step {
    /// YouTube has this many bytes.
    Received(u64),
    Done(Uploaded),
    /// The session no longer exists.
    Expired,
    /// Worth trying again.
    Failed(AppError),
}

/// Sends a chunk, or with `None` asks how far the session is.
fn put(api: &Api, session: &str, size: u64, chunk: Option<(u64, &[u8])>) -> Result<Step, AppError> {
    let (range, body) = match chunk {
        Some((from, bytes)) => (
            format!("bytes {from}-{}/{size}", from + bytes.len() as u64 - 1),
            bytes,
        ),
        None => (format!("bytes */{size}"), &[][..]),
    };
    let response = match agent()
        .put(session)
        .header("Authorization", &api.bearer()?)
        .header("Content-Range", &range)
        .send(body)
    {
        Ok(response) => response,
        Err(error) => return Ok(Step::Failed(unreachable(error))),
    };
    match response.status().as_u16() {
        200 | 201 => video(response).map(Step::Done),
        308 => {
            // `bytes=0-N`; absent when nothing has arrived.
            let received = header(&response, "range")
                .and_then(|range| range.rsplit_once('-')?.1.parse::<u64>().ok())
                .map_or(0, |last| last + 1);
            Ok(Step::Received(received.min(size)))
        }
        404 | 410 => Ok(Step::Expired),
        500..=599 => Ok(Step::Failed(refusal(response))),
        _ => Err(refusal(response)),
    }
}

fn check(cancel: &AtomicBool) -> Result<(), AppError> {
    if cancel.load(Ordering::SeqCst) {
        Err(AppError::new(CANCELLED_MESSAGE))
    } else {
        Ok(())
    }
}

/// Waits unless cancelled meanwhile.
fn pause(total: Duration, cancel: &AtomicBool) -> Result<(), AppError> {
    let slice = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() {
        check(cancel)?;
        std::thread::sleep(left.min(slice));
        left = left.saturating_sub(slice);
    }
    check(cancel)
}

/// Sends the file from wherever the session is. `None` means the session expired and a new one is needed.
/// `cancel` is checked between chunks.
pub fn send(
    api: &Api,
    session: &str,
    file: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<Option<Uploaded>, AppError> {
    let read_failed =
        |e: std::io::Error| AppError::new(format!("cannot read {}: {e}", file.display()));
    let mut source = File::open(file).map_err(read_failed)?;
    let size = source.metadata().map_err(read_failed)?.len();
    // `None`: the position is unknown and has to be asked.
    let mut offset: Option<u64> = None;
    let mut failures = 0;
    let mut buffer = Vec::new();
    loop {
        check(cancel)?;
        let step = match offset {
            Some(from) if from < size => {
                source.seek(SeekFrom::Start(from)).map_err(read_failed)?;
                buffer.clear();
                (&mut source)
                    .take(CHUNK_BYTES)
                    .read_to_end(&mut buffer)
                    .map_err(read_failed)?;
                put(api, session, size, Some((from, &buffer)))?
            }
            _ => put(api, session, size, None)?,
        };
        match step {
            Step::Done(uploaded) => {
                on_progress(size, size);
                return Ok(Some(uploaded));
            }
            Step::Received(received) => {
                // No progress after sending counts as a failure, so a stuck session cannot loop for ever.
                if offset.is_some_and(|from| received <= from) {
                    failures += 1;
                    if failures >= ATTEMPTS {
                        return Err(AppError::new("YouTube is not accepting the video data"));
                    }
                    pause(api.backoff * 2_u32.pow(failures - 1), cancel)?;
                } else if offset.is_some() {
                    failures = 0;
                }
                offset = Some(received);
                on_progress(received, size);
            }
            Step::Expired => return Ok(None),
            Step::Failed(error) => {
                failures += 1;
                if failures >= ATTEMPTS {
                    return Err(error);
                }
                pause(api.backoff * 2_u32.pow(failures - 1), cancel)?;
                offset = None;
            }
        }
    }
}

/// Adds the English subtitle track. A track that is already there counts as done.
pub fn captions(api: &Api, video_id: &str, srt: &str) -> Result<(), AppError> {
    let snippet = json!({"snippet": {"videoId": video_id, "language": "en", "name": "English"}});
    let body = format!(
        "--{BOUNDARY}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{snippet}\r\n\
         --{BOUNDARY}\r\nContent-Type: application/octet-stream\r\n\r\n{srt}\r\n--{BOUNDARY}--\r\n"
    );
    let url = format!(
        "{}/upload/youtube/v3/captions?uploadType=multipart&part=snippet",
        api.base
    );
    let response = agent()
        .post(&url)
        .header("Authorization", &api.bearer()?)
        .header(
            "Content-Type",
            &format!("multipart/related; boundary={BOUNDARY}"),
        )
        .send(body)
        .map_err(unreachable)?;
    match response.status().as_u16() {
        200 | 201 | 409 => Ok(()),
        _ => Err(refusal(response)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::Server;
    use super::*;

    const DONE: &str =
        r#"{"id":"vid","status":{"privacyStatus":"private","publishAt":"2026-12-01T10:00:00Z"}}"#;

    fn api(server: &Server) -> Api {
        Api {
            base: server.url.clone(),
            token: Box::new(|| Ok("tok".into())),
            backoff: Duration::from_millis(1),
        }
    }

    fn video_file(name: &str, size: u64) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("audiobooks-up-{name}-{}", std::process::id()));
        File::create(&path).unwrap().set_len(size).unwrap();
        path
    }

    fn run(server: &Server, file: &Path) -> Result<Option<Uploaded>, AppError> {
        let session = format!("{}/session", server.url);
        send(
            &api(server),
            &session,
            file,
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
    }

    #[test]
    fn start_sends_the_metadata_and_returns_the_session_address() {
        let server = Server::start(&[("200 OK", "Location: {url}/session", "")]);
        let session = start(&api(&server), &json!({"snippet": {"title": "T"}}), 42).unwrap();
        assert_eq!(session, format!("{}/session", server.url));
        let request = &server.requests()[0];
        assert!(
            request
                .line
                .starts_with("POST /upload/youtube/v3/videos?uploadType=resumable")
        );
        assert_eq!(request.header("x-upload-content-length"), "42");
        assert_eq!(request.header("authorization"), "bearer tok");
        assert!(String::from_utf8_lossy(&request.body).contains(r#""title":"T""#));
    }

    #[test]
    fn a_file_goes_up_in_chunks_with_progress_and_the_returned_status() {
        let size = CHUNK_BYTES + 10;
        let first = format!("Range: bytes=0-{}", CHUNK_BYTES - 1);
        let server = Server::start(&[
            ("308 Resume Incomplete", "", ""),
            ("308 Resume Incomplete", &first, ""),
            ("201 Created", "", DONE),
        ]);
        let file = video_file("chunks", size);
        let mut seen = Vec::new();
        let session = format!("{}/session", server.url);
        let uploaded = send(
            &api(&server),
            &session,
            &file,
            &AtomicBool::new(false),
            &mut |sent, _| seen.push(sent),
        )
        .unwrap()
        .unwrap();
        assert_eq!(uploaded.video_id, "vid");
        assert_eq!(uploaded.privacy.as_deref(), Some("private"));
        assert_eq!(uploaded.publish_at.as_deref(), Some("2026-12-01T10:00:00Z"));
        assert_eq!(seen, [0, CHUNK_BYTES, size]);
        let requests = server.requests();
        assert_eq!(
            requests[0].header("content-range"),
            format!("bytes */{size}")
        );
        assert_eq!(
            requests[2].header("content-range"),
            format!("bytes {CHUNK_BYTES}-{}/{size}", size - 1)
        );
        assert_eq!(requests[2].body.len(), 10);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn after_a_server_error_it_asks_where_it_was_and_resumes_from_there() {
        let server = Server::start(&[
            ("308 Resume Incomplete", "", ""),
            ("503 Unavailable", "", ""),
            ("308 Resume Incomplete", "Range: bytes=0-3", ""),
            ("200 OK", "", DONE),
        ]);
        let file = video_file("resume", 10);
        assert!(run(&server, &file).unwrap().is_some());
        let requests = server.requests();
        assert_eq!(requests[2].header("content-range"), "bytes */10");
        assert_eq!(requests[3].header("content-range"), "bytes 4-9/10");
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_finished_session_is_not_uploaded_twice_and_an_expired_one_is_reported() {
        let file = video_file("done", 10);
        let server = Server::start(&[("200 OK", "", DONE)]);
        assert_eq!(run(&server, &file).unwrap().unwrap().video_id, "vid");
        assert!(server.requests()[0].body.is_empty());
        assert_eq!(server.requests()[0].header("content-length"), "0");
        let server = Server::start(&[("404 Not Found", "", "")]);
        assert_eq!(run(&server, &file).unwrap(), None);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn it_gives_up_after_repeated_failures_and_stops_when_cancelled() {
        let file = video_file("fail", 10);
        let server = Server::start(&[("500 Oops", "", ""); ATTEMPTS as usize]);
        assert!(run(&server, &file).unwrap_err().to_string().contains("500"));
        let server = Server::start(&[("308 Resume Incomplete", "", "")]);
        let cancel = AtomicBool::new(false);
        let session = format!("{}/session", server.url);
        let error = send(&api(&server), &session, &file, &cancel, &mut |_, _| {
            cancel.store(true, Ordering::SeqCst)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), CANCELLED_MESSAGE);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn captions_go_as_one_multipart_request_and_an_existing_track_is_fine() {
        let server = Server::start(&[("200 OK", "", "{}"), ("409 Conflict", "", "")]);
        captions(&api(&server), "vid", "1\nsrt text").unwrap();
        captions(&api(&server), "vid", "1\nsrt text").unwrap();
        let body = String::from_utf8(server.requests()[0].body.clone()).unwrap();
        assert!(
            body.contains(r#""videoId":"vid""#) && body.contains("1\nsrt text"),
            "{body}"
        );
    }

    #[test]
    fn refusals_are_explained() {
        for (status, body, expected) in [
            (
                "403 Forbidden",
                r#"{"error":{"errors":[{"reason":"quotaExceeded"}]}}"#,
                "quota",
            ),
            ("401 Unauthorized", "{}", "connect the account again"),
            (
                "400 Bad Request",
                r#"{"error":{"message":"Bad title"}}"#,
                "Bad title",
            ),
        ] {
            let server = Server::start(&[(status, "", body)]);
            let error = captions(&api(&server), "v", "s").unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
}
