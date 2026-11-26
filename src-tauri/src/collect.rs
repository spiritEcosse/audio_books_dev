//! Starting and collecting generations in two phases, so that waiting is the app's job and not Claude's.
//!
//! A *start* run (headless Claude, ElevenLabs tools only) launches every generation exactly once and answers with the
//! flow and session ids at once. The ids are saved on the `scene_assets` rows, so a paid generation can always be
//! collected later and is never started a second time. A *collect* run is a tiny Claude call that asks the status of
//! the saved sessions once; the app repeats it every `POLL_INTERVAL` until everything is done or `MAX_ROUNDS` ran out.
//! What is still running then stays "in progress" with its ids, to be collected with one click.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::claude;
use crate::error::AppError;
use crate::media::STATUS_REQUESTED;
use crate::media_queue::{self, CONNECTOR_TOOLS, ItemResult, MEDIA_MODEL, Outcome, Saved};

/// Pause between two status checks.
pub const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Status checks before giving up for now: about ten minutes. Giving up leaves the paid generation collectable.
pub const MAX_ROUNDS: u32 = 40;
/// A flaky Claude call is retried; this many in a row end the collecting for now.
const MAX_CONSECUTIVE_FAILURES: u32 = 4;
const COLLECT_BASE_TURNS: u32 = 8;
const COLLECT_TURNS_PER_ITEM: u32 = 1;
/// Turn budget of a start run before the per-item share: the flow, the uploads, one generate call per item, the reply.
pub const START_BASE_TURNS: u32 = 14;

pub const COLLECT_SYSTEM: &str = r#"You operate ElevenLabs tools only and never any other tool. Follow the steps exactly.
1. Call creative_get_flow_run_status ONCE for every distinct flow_id in the task, with the session_ids of that flow's items. Make these calls in parallel in one step. Never call any generate, edit or flow-creating tool: that would be charged again. Never call the status tool twice for the same flow.
2. Reply with ONLY one JSON object and nothing else: {"results":[{"position":<int>,"state":"completed"|"failed"|"pending","url":<the generation's content_url or null>,"credits":<its price.credits or null>,"prompt":<the generation's prompt exactly as the status tool returned it, or null>,"message":<the failure reason when state is failed, else null>}]}. For each item look at the generation of its session_id: state "completed" when its status is completed, "failed" when it failed, otherwise "pending". An item whose session is missing from the status answer is "pending"."#;

/// One generation that was launched: where to ask for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub position: i64,
    pub flow_id: String,
    pub session_id: String,
}

/// What a start run reported: what was launched and what could not be.
#[derive(Debug, Clone, PartialEq)]
pub struct StartReport {
    pub started: Vec<Started>,
    pub failed: Vec<(i64, String)>,
}

/// A launched generation whose result has not been collected.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingItem {
    pub position: i64,
    pub flow_id: String,
    pub session_id: String,
    /// The prompt or narration text that was asked for; a result that came back altered is flagged.
    pub expected_text: String,
    pub estimate_credits: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Collected {
    Completed(Outcome),
    Failed(String),
    Pending,
}

/// What collecting achieved: the finished items and the ones still running.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub finished: Vec<ItemResult>,
    pub pending: Vec<PendingItem>,
}

pub fn parse_started(text: &str) -> Result<StartReport, AppError> {
    let json = media_queue::extract_json(text)?;
    let id = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let started: Vec<Started> = json["started"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Started {
                position: s["position"].as_i64()?,
                flow_id: id(&s["flow_id"])?,
                session_id: id(&s["session_id"])?,
            })
        })
        .collect();
    let failed: Vec<(i64, String)> = json["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                e["position"].as_i64()?,
                e["message"].as_str().unwrap_or("unknown error").to_owned(),
            ))
        })
        .collect();
    if started.is_empty() && failed.is_empty() {
        return Err(AppError::new(
            "Claude reported neither a started generation nor an error",
        ));
    }
    Ok(StartReport { started, failed })
}

pub fn parse_collected(text: &str) -> Result<Vec<(i64, Collected)>, AppError> {
    let json = media_queue::extract_json(text)?;
    Ok(json["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let position = r["position"].as_i64()?;
            let collected = match r["state"].as_str() {
                Some("completed") => match r["url"].as_str().filter(|u| !u.is_empty()) {
                    Some(url) => Collected::Completed(Outcome {
                        position,
                        url: Some(url.to_owned()),
                        credits: r["credits"].as_f64(),
                        prompt: r["prompt"].as_str().map(str::to_owned),
                        error: None,
                    }),
                    None => Collected::Pending,
                },
                Some("failed") => Collected::Failed(
                    r["message"]
                        .as_str()
                        .unwrap_or("the generation failed")
                        .to_owned(),
                ),
                _ => Collected::Pending,
            };
            Some((position, collected))
        })
        .collect())
}

/// Asks Claude (once) for the state of the generations. Blocking.
pub fn fetch_status(cwd: &Path, items: &[PendingItem]) -> Result<Vec<(i64, Collected)>, AppError> {
    let list: Vec<Value> = items
        .iter()
        .map(
            |i| json!({ "position": i.position, "flow_id": i.flow_id, "session_id": i.session_id }),
        )
        .collect();
    let prompt = media_queue::items_prompt("Report the state of these generations", list);
    let turns = COLLECT_BASE_TURNS + COLLECT_TURNS_PER_ITEM * items.len() as u32;
    let reply = claude::run_connector(
        cwd,
        MEDIA_MODEL,
        COLLECT_SYSTEM,
        &prompt,
        CONNECTOR_TOOLS,
        turns,
    )?;
    parse_collected(&reply.text)
}

/// Checks the pending items again and again, finishing each as soon as it is done. The status check, the way a
/// result is saved and the waiting are passed in, so the loop itself is plain logic.
pub fn collect_until_done(
    items: Vec<PendingItem>,
    mut fetch: impl FnMut(&[PendingItem]) -> Result<Vec<(i64, Collected)>, AppError>,
    mut finish: impl FnMut(&PendingItem, &Outcome) -> Result<Saved, String>,
    mut wait: impl FnMut(),
    max_rounds: u32,
) -> Collection {
    let mut pending = items;
    let mut finished: Vec<ItemResult> = Vec::new();
    let mut failures = 0;
    for round in 0..max_rounds {
        if pending.is_empty() {
            break;
        }
        if round > 0 {
            wait();
        }
        let states = match fetch(&pending) {
            Ok(states) => {
                failures = 0;
                states
            }
            Err(_) => {
                failures += 1;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    break;
                }
                continue;
            }
        };
        let mut still_pending = Vec::new();
        for item in pending {
            match states
                .iter()
                .find(|(position, _)| *position == item.position)
                .map(|(_, state)| state)
            {
                Some(Collected::Completed(outcome)) => {
                    finished.push(ItemResult {
                        position: item.position,
                        saved: finish(&item, outcome),
                    });
                }
                Some(Collected::Failed(message)) => {
                    finished.push(ItemResult {
                        position: item.position,
                        saved: Err(message.clone()),
                    });
                }
                _ => still_pending.push(item),
            }
        }
        pending = still_pending;
    }
    Collection { finished, pending }
}

/// The real thing: Claude for the status, downloads into `dir`, a real pause between rounds. Blocking, up to about
/// ten minutes.
pub fn collect_pending(cwd: &Path, dir: &Path, kind: &str, items: Vec<PendingItem>) -> Collection {
    collect_until_done(
        items,
        |pending| fetch_status(cwd, pending),
        |item, outcome| {
            media_queue::save_outcome(
                kind,
                item.position,
                &item.expected_text,
                item.estimate_credits,
                Some(outcome),
                dir,
            )
        },
        || std::thread::sleep(POLL_INTERVAL),
        MAX_ROUNDS,
    )
}

/// Remembers where to collect each launched generation. From here on the item is paid for and never launched again.
pub fn save_started(
    conn: &Connection,
    story_id: i64,
    kind: &str,
    started: &[Started],
) -> Result<(), AppError> {
    for item in started {
        conn.execute(
            "UPDATE scene_assets SET session_id = ?4, flow_id = ?5
             WHERE story_id = ?1 AND position = ?2 AND kind = ?3 AND status = 'requested'",
            params![story_id, item.position, kind, item.session_id, item.flow_id],
        )?;
    }
    Ok(())
}

/// Launched generations of a story that are still waiting to be collected.
pub fn pending_items(
    conn: &Connection,
    story_id: i64,
    kind: &str,
) -> Result<Vec<PendingItem>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT position, source, estimate_credits, session_id, flow_id FROM scene_assets
         WHERE story_id = ?1 AND kind = ?2 AND status = ?3 AND started_at IS NOT NULL AND session_id IS NOT NULL
         ORDER BY position",
    )?;
    let rows = stmt.query_map(params![story_id, kind, STATUS_REQUESTED], |row| {
        Ok(PendingItem {
            position: row.get(0)?,
            expected_text: row.get(1)?,
            estimate_credits: row.get(2)?,
            session_id: row.get(3)?,
            flow_id: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Gives up on a paid generation that never arrived, so the item can be generated again (and paid for again).
pub fn abandon(
    conn: &Connection,
    story_id: i64,
    position: i64,
    kind: &str,
) -> Result<(), AppError> {
    let changed = conn.execute(
        "UPDATE scene_assets SET status = 'failed', started_at = NULL, session_id = NULL, flow_id = NULL,
             error = 'abandoned: the paid generation was never collected', updated_at = CURRENT_TIMESTAMP
         WHERE story_id = ?1 AND position = ?2 AND kind = ?3 AND status = 'requested' AND started_at IS NOT NULL",
        params![story_id, position, kind],
    )?;
    if changed == 0 {
        return Err(AppError::new("nothing is being generated for this item"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, NewScene};
    use crate::media::{self, KIND_IMAGE};

    fn item(position: i64) -> PendingItem {
        PendingItem {
            position,
            flow_id: "flow".into(),
            session_id: format!("session-{position}"),
            expected_text: "prompt".into(),
            estimate_credits: Some(5.0),
        }
    }

    fn completed(position: i64) -> (i64, Collected) {
        (
            position,
            Collected::Completed(Outcome {
                position,
                url: Some("https://x.elevenlabsusercontent.com/a.png".into()),
                credits: Some(1.0),
                prompt: None,
                error: None,
            }),
        )
    }

    fn pending(position: i64) -> (i64, Collected) {
        (position, Collected::Pending)
    }

    fn saved(item: &PendingItem, _outcome: &Outcome) -> Result<Saved, String> {
        Ok(Saved {
            path: format!("/m/{}.png", item.position),
            credits: 1.0,
        })
    }

    #[test]
    fn items_are_finished_as_they_complete_and_the_loop_waits_between_rounds() {
        let mut script = vec![
            vec![pending(1), pending(2)],
            vec![completed(1), pending(2)],
            vec![completed(2)],
        ]
        .into_iter();
        let mut waits = 0;

        let collection = collect_until_done(
            vec![item(1), item(2)],
            |_| Ok(script.next().unwrap()),
            saved,
            || waits += 1,
            MAX_ROUNDS,
        );

        assert_eq!(
            collection
                .finished
                .iter()
                .map(|r| r.position)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(collection.pending.is_empty());
        assert_eq!(waits, 2, "no wait before the first check");
    }

    #[test]
    fn the_round_limit_leaves_unfinished_items_pending_with_their_ids() {
        let mut checks = 0;
        let collection = collect_until_done(
            vec![item(1)],
            |_| {
                checks += 1;
                Ok(vec![pending(1)])
            },
            saved,
            || {},
            5,
        );
        assert_eq!(checks, 5);
        assert!(collection.finished.is_empty());
        assert_eq!(collection.pending, [item(1)]);
    }

    #[test]
    fn a_failed_generation_becomes_an_error_result_and_is_not_asked_about_again() {
        let mut script = vec![vec![(1, Collected::Failed("moderated".into()))]].into_iter();
        let collection = collect_until_done(
            vec![item(1)],
            |_| Ok(script.next().unwrap()),
            saved,
            || {},
            MAX_ROUNDS,
        );
        assert_eq!(
            collection.finished,
            [ItemResult {
                position: 1,
                saved: Err("moderated".into())
            }]
        );
        assert!(collection.pending.is_empty());
    }

    #[test]
    fn a_flaky_check_is_retried_but_repeated_failures_stop_the_loop_without_losing_the_item() {
        let mut calls = 0;
        let collection = collect_until_done(
            vec![item(1)],
            |_| {
                calls += 1;
                if calls == 1 {
                    Err(AppError::new("claude exited"))
                } else {
                    Ok(vec![pending(1)])
                }
            },
            saved,
            || {},
            3,
        );
        assert_eq!(calls, 3, "one failure does not stop it");
        assert_eq!(collection.pending.len(), 1);

        let mut always_failing = 0;
        let collection = collect_until_done(
            vec![item(1)],
            |_| {
                always_failing += 1;
                Err(AppError::new("claude exited"))
            },
            saved,
            || {},
            MAX_ROUNDS,
        );
        assert_eq!(always_failing, MAX_CONSECUTIVE_FAILURES);
        assert_eq!(
            collection.pending.len(),
            1,
            "the paid generation stays collectable"
        );
    }

    #[test]
    fn an_item_missing_from_the_answer_stays_pending() {
        let collection = collect_until_done(
            vec![item(1), item(2)],
            |_| Ok(vec![completed(1)]),
            saved,
            || {},
            1,
        );
        assert_eq!(collection.finished.len(), 1);
        assert_eq!(collection.pending, [item(2)]);
    }

    #[test]
    fn a_save_failure_is_reported_for_that_item_only() {
        let collection = collect_until_done(
            vec![item(1), item(2)],
            |_| Ok(vec![completed(1), completed(2)]),
            |item, outcome| {
                if item.position == 1 {
                    Err("download failed".into())
                } else {
                    saved(item, outcome)
                }
            },
            || {},
            1,
        );
        assert_eq!(collection.finished[0].saved, Err("download failed".into()));
        assert!(collection.finished[1].saved.is_ok());
    }

    #[test]
    fn start_answers_are_parsed_and_an_empty_answer_is_an_error() {
        let report = parse_started(
            r#"ok {"started":[{"position":0,"flow_id":"f","session_id":"s0"},{"position":1,"flow_id":"","session_id":"s1"}],
                "errors":[{"position":2,"message":"moderated"}]}"#,
        )
        .unwrap();
        assert_eq!(
            report.started,
            [Started {
                position: 0,
                flow_id: "f".into(),
                session_id: "s0".into()
            }]
        );
        assert_eq!(report.failed, [(2, "moderated".to_owned())]);
        assert!(parse_started(r#"{"started":[],"errors":[]}"#).is_err());
        assert!(parse_started("no json here").is_err());
    }

    #[test]
    fn collect_answers_map_to_states() {
        let states = parse_collected(
            r#"{"results":[
              {"position":0,"state":"completed","url":"https://a/b.png","credits":212.1,"prompt":"p"},
              {"position":1,"state":"completed","url":null},
              {"position":2,"state":"failed","message":"moderated"},
              {"position":3,"state":"generating"}]}"#,
        )
        .unwrap();
        assert!(
            matches!(&states[0], (0, Collected::Completed(o)) if o.url.as_deref() == Some("https://a/b.png"))
        );
        assert_eq!(
            states[1],
            (1, Collected::Pending),
            "completed without a link is not usable yet"
        );
        assert_eq!(states[2], (2, Collected::Failed("moderated".into())));
        assert_eq!(states[3], (3, Collected::Pending));
    }

    #[test]
    fn the_collect_prompt_only_reads_and_the_start_prompt_never_polls() {
        assert!(COLLECT_SYSTEM.contains("creative_get_flow_run_status"));
        assert!(COLLECT_SYSTEM.contains("Never call any generate"));
    }

    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let story = db::create_story(&conn, "t", "body").unwrap();
        let scenes = [NewScene {
            text: "a".into(),
            image_prompt: "draw a".into(),
        }];
        db::replace_scenes(&mut conn, story.id, &scenes).unwrap();
        media::request_asset(&conn, story.id, 0, KIND_IMAGE).unwrap();
        (conn, story.id)
    }

    #[test]
    fn launched_generations_keep_their_ids_until_a_result_is_applied() {
        let (conn, story) = setup();
        media_queue::mark_started(&conn, story, KIND_IMAGE, &[0]).unwrap();
        save_started(
            &conn,
            story,
            KIND_IMAGE,
            &[Started {
                position: 0,
                flow_id: "f".into(),
                session_id: "s".into(),
            }],
        )
        .unwrap();

        let pending = pending_items(&conn, story, KIND_IMAGE).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            (pending[0].flow_id.as_str(), pending[0].session_id.as_str()),
            ("f", "s")
        );
        assert_eq!(pending[0].expected_text, "draw a");

        let results = [ItemResult {
            position: 0,
            saved: Ok(Saved {
                path: "/m/a.png".into(),
                credits: 3.0,
            }),
        }];
        media_queue::apply_results(&conn, story, KIND_IMAGE, &results).unwrap();
        assert!(pending_items(&conn, story, KIND_IMAGE).unwrap().is_empty());
        let ids: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT session_id, flow_id FROM scene_assets WHERE story_id = ?1",
                [story],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            ids,
            (None, None),
            "ids are cleared once the result is saved"
        );
    }

    #[test]
    fn an_item_with_a_saved_session_is_never_queued_or_generated_again() {
        let (conn, story) = setup();
        media_queue::mark_started(&conn, story, KIND_IMAGE, &[0]).unwrap();
        save_started(
            &conn,
            story,
            KIND_IMAGE,
            &[Started {
                position: 0,
                flow_id: "f".into(),
                session_id: "s".into(),
            }],
        )
        .unwrap();

        assert!(
            media::request_asset(&conn, story, 0, KIND_IMAGE).is_err(),
            "re-requesting would pay twice"
        );
        assert!(
            media_queue::queued_rows(&conn, story, KIND_IMAGE)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn abandoning_frees_the_item_and_forgets_the_ids() {
        let (conn, story) = setup();
        assert!(
            abandon(&conn, story, 0, KIND_IMAGE).is_err(),
            "nothing was started yet"
        );
        media_queue::mark_started(&conn, story, KIND_IMAGE, &[0]).unwrap();
        save_started(
            &conn,
            story,
            KIND_IMAGE,
            &[Started {
                position: 0,
                flow_id: "f".into(),
                session_id: "s".into(),
            }],
        )
        .unwrap();

        abandon(&conn, story, 0, KIND_IMAGE).unwrap();

        assert!(pending_items(&conn, story, KIND_IMAGE).unwrap().is_empty());
        media::request_asset(&conn, story, 0, KIND_IMAGE).unwrap();
    }
}
