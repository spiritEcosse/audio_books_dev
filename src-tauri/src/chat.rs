use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::claude;
use crate::db::{Message, NewScene, Scene, Story, StorySettings};
use crate::error::AppError;
use crate::profiles::Profile;

const BASE_SYSTEM_PROMPT: &str = "You are the editing assistant inside an audiobook video app. \
The user chats with you about one story. A story is split into scenes for an illustrated, \
narrated video: each scene's \"text\" is a consecutive passage read aloud (about 20-60 seconds), \
in the story's original order and wording unless the user asks to change it; \"image_prompt\" is \
a vivid English description of one still image for the scene. \
Reply language: match the user's language. \
ALWAYS reply with ONLY one JSON object, no code fences, no text around it, with these keys: \
\"reply\" (your message to the user), \
\"scenes\" (null when scenes stay as they are, otherwise the COMPLETE new list \
[{\"text\": \"...\", \"image_prompt\": \"...\"}, ...] whenever you create or change any), \
\"story_text\" (null normally; only when the user asks you to rewrite or edit the story text \
itself, the COMPLETE new story text — the user must confirm it before it is saved). \
You cannot touch files; the app applies \"scenes\" and, after confirmation, \"story_text\" for you. \
The app may prepend <app_notes> to a user message to tell you what the user did outside the chat \
(accepted or rejected a proposal, restored an older version); treat them as facts.";

const AUDIENCE_GUIDE: &str = include_str!("../prompts/audience.md");

/// Base chat protocol, audience rules and the story's writing profile: the system prompt of every turn.
fn system_prompt(profile: &Profile) -> String {
    format!(
        "{BASE_SYSTEM_PROMPT}\n\n{AUDIENCE_GUIDE}\n\n{}",
        profile.body
    )
}

/// Everything one chat turn needs.
pub struct TurnInput<'a> {
    pub cwd: &'a Path,
    pub story: &'a Story,
    pub scenes: &'a [Scene],
    pub settings: &'a StorySettings,
    pub profile: &'a Profile,
    pub history: &'a [Message],
    pub session_id: Option<&'a str>,
    pub model: &'a str,
    pub user_text: &'a str,
}

#[derive(Deserialize)]
struct Envelope {
    reply: String,
    #[serde(default)]
    scenes: Option<Vec<NewScene>>,
    #[serde(default)]
    story_text: Option<String>,
}

pub struct Turn {
    pub reply: String,
    pub scenes: Option<Vec<NewScene>>,
    pub story_text: Option<String>,
    pub session_id: Option<String>,
}

/// Parses the model's JSON envelope. Plain-text replies (the model ignored the format)
/// are kept as a reply with no scene changes, and an empty scene list never wipes scenes.
pub fn parse_envelope(text: &str) -> ParsedReply {
    let candidate = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => return ParsedReply::plain(text),
    };
    match serde_json::from_str::<Envelope>(candidate) {
        Ok(envelope) => ParsedReply {
            reply: envelope.reply,
            scenes: envelope.scenes.filter(|scenes| !scenes.is_empty()),
            story_text: envelope.story_text.filter(|t| !t.trim().is_empty()),
        },
        Err(_) => ParsedReply::plain(text),
    }
}

pub struct ParsedReply {
    pub reply: String,
    pub scenes: Option<Vec<NewScene>>,
    pub story_text: Option<String>,
}

impl ParsedReply {
    fn plain(text: &str) -> Self {
        Self {
            reply: text.trim().to_owned(),
            scenes: None,
            story_text: None,
        }
    }
}

/// Prefixes a user message with facts about what the user did outside the chat.
pub fn with_notes(notes: &[String], text: &str) -> String {
    if notes.is_empty() {
        return text.to_owned();
    }
    let lines: Vec<String> = notes.iter().map(|n| format!("- {n}")).collect();
    format!("<app_notes>\n{}\n</app_notes>\n{text}", lines.join("\n"))
}

fn state_block(
    story: &Story,
    scenes: &[Scene],
    settings: &StorySettings,
    profile: &Profile,
) -> String {
    let scenes_json = json!(
        scenes
            .iter()
            .map(|s| json!({ "text": s.text, "image_prompt": s.image_prompt }))
            .collect::<Vec<_>>()
    );
    format!(
        "<story_settings audience=\"{}\" target_minutes=\"{}\" profile=\"{}\"/>\n<story title=\"{}\">\n{}\n</story>\n<current_scenes>\n{}\n</current_scenes>",
        settings.age_group,
        settings.target_minutes,
        profile.label,
        story.title,
        story.text,
        scenes_json
    )
}

fn transcript(history: &[Message]) -> String {
    history
        .iter()
        .map(|m| {
            format!(
                "{}: {}",
                if m.role == "user" {
                    "User"
                } else {
                    "Assistant"
                },
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prompt for a brand-new Claude session: full state plus any earlier conversation.
fn fresh_session_prompt(input: &TurnInput<'_>) -> String {
    let mut prompt = state_block(input.story, input.scenes, input.settings, input.profile);
    if !input.history.is_empty() {
        prompt.push_str(&format!(
            "\n<conversation_so_far>\n{}\n</conversation_so_far>",
            transcript(input.history)
        ));
    }
    prompt.push_str(&format!(
        "\n<user_message>\n{}\n</user_message>",
        input.user_text
    ));
    prompt
}

/// Runs one chat turn. Continues the saved session when there is one; if that session no
/// longer exists, rebuilds the context from SQLite history in a new session.
/// `input.history` must not include the new user message. Blocking.
pub fn run_turn(input: &TurnInput<'_>) -> Result<Turn, AppError> {
    let system = system_prompt(input.profile);
    let fresh = |system: &str| {
        claude::run(
            input.cwd,
            input.model,
            system,
            &fresh_session_prompt(input),
            None,
        )
    };
    let reply = match input.session_id {
        Some(id) => match claude::run(input.cwd, input.model, &system, input.user_text, Some(id)) {
            Ok(reply) => reply,
            // Only a vanished session triggers the rebuild; other failures (network,
            // limits) are surfaced instead of silently paying for a full-context retry.
            Err(err) if claude::is_session_missing(&err) => fresh(&system)?,
            Err(err) => return Err(err),
        },
        None => fresh(&system)?,
    };
    let parsed = parse_envelope(&reply.text);
    Ok(Turn {
        reply: parsed.reply,
        scenes: parsed.scenes,
        story_text: parsed.story_text,
        session_id: reply.session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(body: &str) -> Profile {
        Profile {
            id: "p".into(),
            label: "Label".into(),
            builtin: true,
            body: body.into(),
        }
    }

    fn settings() -> StorySettings {
        StorySettings {
            target_minutes: 7,
            age_group: "up_to_5".into(),
            profile: "p".into(),
        }
    }

    #[test]
    fn system_prompt_contains_protocol_audience_rules_and_profile() {
        let prompt = system_prompt(&profile("PROFILE-BODY"));
        assert!(prompt.contains("\"story_text\""));
        assert!(prompt.contains("AUDIENCE AND LENGTH"));
        assert!(prompt.ends_with("PROFILE-BODY"));
    }

    #[test]
    fn parses_reply_with_scenes() {
        let parsed =
            parse_envelope(r#"{"reply":"Готово","scenes":[{"text":"A","image_prompt":"b"}]}"#);
        assert_eq!(parsed.reply, "Готово");
        assert_eq!(parsed.scenes.unwrap().len(), 1);
    }

    #[test]
    fn parses_reply_with_null_scenes() {
        let parsed = parse_envelope(r#"{"reply":"Ок","scenes":null}"#);
        assert_eq!(parsed.reply, "Ок");
        assert!(parsed.scenes.is_none() && parsed.story_text.is_none());
    }

    #[test]
    fn tolerates_code_fence_around_json() {
        let parsed = parse_envelope("```json\n{\"reply\":\"Hi\",\"scenes\":null}\n```");
        assert_eq!(parsed.reply, "Hi");
    }

    #[test]
    fn plain_text_becomes_reply_without_scene_changes() {
        let parsed = parse_envelope("Просто текст");
        assert_eq!(parsed.reply, "Просто текст");
        assert!(parsed.scenes.is_none());
    }

    #[test]
    fn empty_scene_list_never_wipes_scenes() {
        let parsed = parse_envelope(r#"{"reply":"x","scenes":[]}"#);
        assert!(parsed.scenes.is_none());
    }

    #[test]
    fn parses_proposed_story_text_and_ignores_blank_one() {
        let with = parse_envelope(r#"{"reply":"r","scenes":null,"story_text":"New text"}"#);
        assert_eq!(with.story_text.as_deref(), Some("New text"));
        let blank = parse_envelope(r#"{"reply":"r","story_text":"  "}"#);
        assert!(blank.story_text.is_none());
    }

    #[test]
    fn notes_are_prefixed_only_when_present() {
        assert_eq!(with_notes(&[], "hi"), "hi");
        let text = with_notes(&["did X".into()], "hi");
        assert!(
            text.starts_with("<app_notes>") && text.contains("- did X") && text.ends_with("hi")
        );
    }

    #[test]
    fn fresh_prompt_includes_settings_story_history_and_message() {
        let story = Story {
            id: 1,
            title: "T".into(),
            text: "body".into(),
        };
        let history = [Message {
            id: 1,
            role: "user".into(),
            content: "earlier".into(),
            scenes_changed: false,
            proposal: None,
        }];
        let (settings, profile) = (settings(), profile("x"));
        let input = TurnInput {
            cwd: Path::new("."),
            story: &story,
            scenes: &[],
            settings: &settings,
            profile: &profile,
            history: &history,
            session_id: None,
            model: "m",
            user_text: "now",
        };

        let prompt = fresh_session_prompt(&input);

        assert!(prompt.contains("audience=\"up_to_5\"") && prompt.contains("target_minutes=\"7\""));
        assert!(
            prompt.contains("body") && prompt.contains("User: earlier") && prompt.contains("now")
        );
    }
}
