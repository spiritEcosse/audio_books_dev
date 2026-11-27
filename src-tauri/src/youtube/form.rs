//! What the user chose for a video: checked against YouTube's limits and turned into the upload metadata.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::AppError;

const MAX_TITLE_CHARS: usize = 100;
const MAX_DESCRIPTION_BYTES: usize = 5000;
const MAX_TAGS_CHARS: usize = 500;
const LANGUAGE: &str = "en";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Private,
    Unlisted,
    Public,
    Scheduled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Form {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub visibility: Visibility,
    /// Unix seconds; used when `visibility` is `scheduled`.
    pub publish_at: Option<i64>,
    /// No default: the user has to say.
    pub made_for_kids: Option<bool>,
    pub synthetic_media: Option<bool>,
    pub subtitles: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            tags: Vec::new(),
            visibility: Visibility::default(),
            publish_at: None,
            made_for_kids: None,
            synthetic_media: None,
            subtitles: true,
        }
    }
}

/// YouTube counts the commas between tags, and quotes around a tag with a space.
fn tags_chars(tags: &[String]) -> usize {
    let each: usize = tags
        .iter()
        .map(|tag| tag.chars().count() + if tag.contains(' ') { 2 } else { 0 })
        .sum();
    each + tags.len().saturating_sub(1)
}

/// `YYYY-MM-DDTHH:MM:SSZ` (the civil-from-days algorithm of H. Hinnant).
fn rfc3339(unix: i64) -> String {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

impl Form {
    /// The body of `videos.insert`, or what the user has to fix first. `now` is in Unix seconds.
    pub fn metadata(&self, now: i64) -> Result<Value, AppError> {
        let refuse = |reason: &str| Err(AppError::new(reason));
        let title = self.title.trim();
        if title.is_empty() {
            return refuse("enter a title");
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return refuse("the title is longer than 100 characters");
        }
        if self.description.len() > MAX_DESCRIPTION_BYTES {
            return refuse("the description is longer than 5000 bytes");
        }
        let texts = [title, self.description.as_str()];
        if texts
            .into_iter()
            .chain(self.tags.iter().map(String::as_str))
            .any(|text| text.contains(['<', '>']))
        {
            return refuse("YouTube does not accept < or > in the title, description or tags");
        }
        if tags_chars(&self.tags) > MAX_TAGS_CHARS {
            return refuse("the tags are longer than 500 characters together");
        }
        let Some(made_for_kids) = self.made_for_kids else {
            return refuse("say whether the video is made for kids");
        };
        let Some(synthetic) = self.synthetic_media else {
            return refuse("say whether the video has altered or synthetic content");
        };
        let privacy = match self.visibility {
            Visibility::Private | Visibility::Scheduled => "private",
            Visibility::Unlisted => "unlisted",
            Visibility::Public => "public",
        };
        let mut status = json!({
            "privacyStatus": privacy,
            "selfDeclaredMadeForKids": made_for_kids,
            "containsSyntheticMedia": synthetic,
        });
        if self.visibility == Visibility::Scheduled {
            let Some(at) = self.publish_at.filter(|at| *at > now) else {
                return refuse("choose a publishing time in the future");
            };
            status["publishAt"] = json!(rfc3339(at));
        }
        Ok(json!({
            "snippet": {
                "title": title,
                "description": self.description,
                "tags": self.tags,
                "defaultLanguage": LANGUAGE,
                "defaultAudioLanguage": LANGUAGE,
            },
            "status": status,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> Form {
        Form {
            title: " The Fox ".into(),
            made_for_kids: Some(true),
            synthetic_media: Some(false),
            ..Form::default()
        }
    }

    fn error(form: &Form) -> String {
        form.metadata(1000).unwrap_err().to_string()
    }

    #[test]
    fn metadata_carries_the_choices_with_english_and_private_by_default() {
        let metadata = filled().metadata(1000).unwrap();
        assert_eq!(metadata["snippet"]["title"], "The Fox");
        assert_eq!(metadata["snippet"]["defaultAudioLanguage"], "en");
        assert_eq!(metadata["status"]["privacyStatus"], "private");
        assert_eq!(metadata["status"]["selfDeclaredMadeForKids"], true);
        assert_eq!(metadata["status"]["containsSyntheticMedia"], false);
        assert!(metadata["status"].get("publishAt").is_none());
        assert!(Form::default().subtitles);
    }

    #[test]
    fn scheduling_is_private_until_a_future_time() {
        let mut form = Form {
            visibility: Visibility::Scheduled,
            publish_at: Some(1_000),
            ..filled()
        };
        assert!(error(&form).contains("future"));
        form.publish_at = Some(1_795_780_800);
        let status = form.metadata(1000).unwrap()["status"].clone();
        assert_eq!(status["privacyStatus"], "private");
        assert_eq!(status["publishAt"], "2026-11-27T12:00:00Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_709_251_199), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn the_youtube_limits_and_required_answers_are_checked() {
        let with = |change: fn(&mut Form)| {
            let mut form = filled();
            change(&mut form);
            error(&form)
        };
        assert!(with(|f| f.title = "  ".into()).contains("title"));
        assert!(with(|f| f.title = "é".repeat(101)).contains("100"));
        assert!(with(|f| f.description = "é".repeat(2501)).contains("5000"));
        assert!(with(|f| f.tags = vec!["<b>".into()]).contains("< or >"));
        assert!(with(|f| f.made_for_kids = None).contains("kids"));
        assert!(with(|f| f.synthetic_media = None).contains("synthetic"));
        assert_eq!(tags_chars(&["a b".into(), "cd".into()]), 8);
        assert!(with(|f| f.tags = vec!["x".repeat(250), "y".repeat(250)]).contains("500"));
    }
}
