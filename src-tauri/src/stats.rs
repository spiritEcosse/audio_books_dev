use serde::Serialize;

use crate::db::{Scene, Story, StorySettings};

#[derive(Debug, Serialize, PartialEq)]
pub struct StoryStats {
    pub words: usize,
    pub characters: usize,
    pub estimated_minutes: f64,
    /// True when measured on the scenes, false when there are none and the story text is used.
    pub from_scenes: bool,
}

/// Slower narration for younger listeners.
pub fn words_per_minute(age_group: &str) -> f64 {
    match age_group {
        "up_to_5" => 100.0,
        "up_to_10" => 120.0,
        "up_to_15" => 140.0,
        _ => 150.0,
    }
}

fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

pub fn estimate(story: &Story, scenes: &[Scene], settings: &StorySettings) -> StoryStats {
    let from_scenes = !scenes.is_empty();
    let text = if from_scenes {
        scenes.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ")
    } else {
        story.text.clone()
    };
    let words = text.split_whitespace().count();
    let minutes = words as f64 / words_per_minute(&settings.age_group);
    StoryStats {
        words,
        characters: text.chars().filter(|c| !c.is_whitespace()).count(),
        estimated_minutes: round_tenth(minutes),
        from_scenes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn story(text: &str) -> Story {
        Story { id: 1, title: "t".into(), text: text.into() }
    }

    fn settings(age: &str) -> StorySettings {
        StorySettings { target_minutes: 10, age_group: age.into(), profile: "fairy_tale".into() }
    }

    fn scene(text: &str) -> Scene {
        Scene { id: 1, position: 0, text: text.into(), image_prompt: "p".into() }
    }

    #[test]
    fn measures_story_text_when_there_are_no_scenes() {
        let stats = estimate(&story(&"слово ".repeat(120)), &[], &settings("up_to_10"));
        assert_eq!((stats.words, stats.estimated_minutes, stats.from_scenes), (120, 1.0, false));
    }

    #[test]
    fn prefers_scenes_over_story_text() {
        let stats = estimate(&story("a b c d e f"), &[scene("one two")], &settings("up_to_10"));
        assert_eq!((stats.words, stats.from_scenes), (2, true));
    }

    #[test]
    fn younger_audiences_narrate_slower() {
        let text = "слово ".repeat(100);
        let young = estimate(&story(&text), &[], &settings("up_to_5")).estimated_minutes;
        let adult = estimate(&story(&text), &[], &settings("adult")).estimated_minutes;
        assert!(young > adult);
    }

    #[test]
    fn counts_characters_without_whitespace() {
        let stats = estimate(&story("ab cd"), &[], &settings("adult"));
        assert_eq!(stats.characters, 4);
    }
}
