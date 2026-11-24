use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::claude;
use crate::db::{Message, NewScene, Scene, Story};
use crate::error::AppError;

const SYSTEM_PROMPT: &str = "You are the editing assistant inside an audiobook video app. \
The user chats with you about one story. A story is split into scenes for an illustrated, \
narrated video: each scene's \"text\" is a consecutive passage read aloud (about 20-60 seconds), \
in the story's original order and wording unless the user asks to change it; \"image_prompt\" is \
a vivid English description of one still image for the scene. \
Reply language: match the user's language. \
ALWAYS reply with ONLY one JSON object, no code fences, no text around it: \
{\"reply\": \"<your message to the user>\", \"scenes\": null} when scenes stay as they are, or \
{\"reply\": \"<message>\", \"scenes\": [{\"text\": \"...\", \"image_prompt\": \"...\"}, ...]} \
with the COMPLETE new list of scenes whenever you create or change any. \
You cannot touch files; the app applies \"scenes\" for you.";

#[derive(Deserialize)]
struct Envelope {
    reply: String,
    #[serde(default)]
    scenes: Option<Vec<NewScene>>,
}

pub struct Turn {
    pub reply: String,
    pub scenes: Option<Vec<NewScene>>,
    pub session_id: Option<String>,
}

/// Parses the model's JSON envelope. Plain-text replies (the model ignored the format)
/// are kept as a reply with no scene changes, and an empty scene list never wipes scenes.
pub fn parse_envelope(text: &str) -> (String, Option<Vec<NewScene>>) {
    let candidate = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => return (text.trim().to_owned(), None),
    };
    match serde_json::from_str::<Envelope>(candidate) {
        Ok(envelope) => (envelope.reply, envelope.scenes.filter(|scenes| !scenes.is_empty())),
        Err(_) => (text.trim().to_owned(), None),
    }
}

fn state_block(story: &Story, scenes: &[Scene]) -> String {
    let scenes_json = json!(
        scenes
            .iter()
            .map(|s| json!({ "text": s.text, "image_prompt": s.image_prompt }))
            .collect::<Vec<_>>()
    );
    format!(
        "<story title=\"{}\">\n{}\n</story>\n<current_scenes>\n{}\n</current_scenes>",
        story.title, story.text, scenes_json
    )
}

fn transcript(history: &[Message]) -> String {
    history
        .iter()
        .map(|m| format!("{}: {}", if m.role == "user" { "User" } else { "Assistant" }, m.content))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prompt for a brand-new Claude session: full state plus any earlier conversation.
fn fresh_session_prompt(story: &Story, scenes: &[Scene], history: &[Message], user_text: &str) -> String {
    let mut prompt = state_block(story, scenes);
    if !history.is_empty() {
        prompt.push_str(&format!("\n<conversation_so_far>\n{}\n</conversation_so_far>", transcript(history)));
    }
    prompt.push_str(&format!("\n<user_message>\n{user_text}\n</user_message>"));
    prompt
}

/// Runs one chat turn. Continues the saved session when there is one; if that session no
/// longer exists, rebuilds the context from SQLite history in a new session.
/// `history` must not include the new user message. Blocking.
pub fn run_turn(
    cwd: &Path,
    story: &Story,
    scenes: &[Scene],
    history: &[Message],
    session_id: Option<&str>,
    user_text: &str,
) -> Result<Turn, AppError> {
    let reply = match session_id {
        Some(id) => match claude::run(cwd, SYSTEM_PROMPT, user_text, Some(id)) {
            Ok(reply) => reply,
            // Only a vanished session triggers the rebuild; other failures (network,
            // limits) are surfaced instead of silently paying for a full-context retry.
            Err(err) if claude::is_session_missing(&err) => claude::run(
                cwd,
                SYSTEM_PROMPT,
                &fresh_session_prompt(story, scenes, history, user_text),
                None,
            )?,
            Err(err) => return Err(err),
        },
        None => claude::run(cwd, SYSTEM_PROMPT, &fresh_session_prompt(story, scenes, history, user_text), None)?,
    };
    let (text, new_scenes) = parse_envelope(&reply.text);
    Ok(Turn { reply: text, scenes: new_scenes, session_id: reply.session_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reply_with_scenes() {
        let (reply, scenes) =
            parse_envelope(r#"{"reply":"Готово","scenes":[{"text":"A","image_prompt":"b"}]}"#);
        assert_eq!(reply, "Готово");
        assert_eq!(scenes.unwrap().len(), 1);
    }

    #[test]
    fn parses_reply_with_null_scenes() {
        let (reply, scenes) = parse_envelope(r#"{"reply":"Ок","scenes":null}"#);
        assert_eq!(reply, "Ок");
        assert!(scenes.is_none());
    }

    #[test]
    fn tolerates_code_fence_around_json() {
        let (reply, _) = parse_envelope("```json\n{\"reply\":\"Hi\",\"scenes\":null}\n```");
        assert_eq!(reply, "Hi");
    }

    #[test]
    fn plain_text_becomes_reply_without_scene_changes() {
        let (reply, scenes) = parse_envelope("Просто текст");
        assert_eq!(reply, "Просто текст");
        assert!(scenes.is_none());
    }

    #[test]
    fn empty_scene_list_never_wipes_scenes() {
        let (_, scenes) = parse_envelope(r#"{"reply":"x","scenes":[]}"#);
        assert!(scenes.is_none());
    }

    #[test]
    fn fresh_prompt_includes_story_history_and_message() {
        let story = Story { id: 1, title: "T".into(), text: "body".into() };
        let history = [Message { id: 1, role: "user".into(), content: "earlier".into(), scenes_changed: false }];

        let prompt = fresh_session_prompt(&story, &[], &history, "now");

        assert!(prompt.contains("body") && prompt.contains("User: earlier") && prompt.contains("now"));
    }
}
