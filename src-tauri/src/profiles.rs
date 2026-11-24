//! Writing profiles: swappable guidance blocks that are added to Claude's system prompt per story.
//! Built-ins ship with the app; users can add their own `.md` files to the profiles folder.

use std::path::Path;

pub const DEFAULT_PROFILE: &str = "fairy_tale";
const MAX_CUSTOM_PROFILE_BYTES: u64 = 20_000;
const CUSTOM_PREFIX: &str = "custom:";

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub builtin: bool,
    pub body: String,
}

fn builtin_profiles() -> Vec<Profile> {
    vec![
        Profile {
            id: DEFAULT_PROFILE.into(),
            label: "Детские сказки".into(),
            builtin: true,
            body: include_str!("../prompts/profiles/fairy_tale.md").into(),
        },
        Profile {
            id: "novel".into(),
            label: "Роман".into(),
            builtin: true,
            body: include_str!("../prompts/profiles/novel.md").into(),
        },
    ]
}

fn label_from(body: &str, fallback: &str) -> String {
    body.lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn load_custom(dir: &Path) -> Vec<Profile> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut profiles: Vec<Profile> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            if path.extension()?.to_str()? != "md" {
                return None;
            }
            if entry.metadata().ok()?.len() > MAX_CUSTOM_PROFILE_BYTES {
                eprintln!("profile {stem}.md skipped: larger than {MAX_CUSTOM_PROFILE_BYTES} bytes");
                return None;
            }
            let body = std::fs::read_to_string(&path)
                .map_err(|e| eprintln!("profile {stem}.md unreadable: {e}"))
                .ok()?;
            Some(Profile {
                label: label_from(&body, &stem),
                id: format!("{CUSTOM_PREFIX}{stem}"),
                builtin: false,
                body,
            })
        })
        .collect();
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    profiles
}

pub fn list(custom_dir: &Path) -> Vec<Profile> {
    let mut all = builtin_profiles();
    all.extend(load_custom(custom_dir));
    all
}

pub fn find(custom_dir: &Path, id: &str) -> Option<Profile> {
    list(custom_dir).into_iter().find(|p| p.id == id)
}

/// The requested profile, or the default one when it no longer exists (e.g. a deleted custom file).
pub fn resolve(custom_dir: &Path, id: &str) -> Profile {
    find(custom_dir, id)
        .or_else(|| find(custom_dir, DEFAULT_PROFILE))
        .expect("default profile is built in")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("audiobooks-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_builtins_first_then_custom_with_heading_label() {
        let dir = temp_dir("custom");
        std::fs::write(dir.join("poetry.md"), "# Стихи\nWrite rhymed verse.").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let ids: Vec<_> = list(&dir).into_iter().map(|p| p.id).collect();

        assert_eq!(ids, ["fairy_tale", "novel", "custom:poetry"]);
        assert_eq!(find(&dir, "custom:poetry").unwrap().label, "Стихи");
    }

    #[test]
    fn oversized_custom_profile_is_skipped() {
        let dir = temp_dir("big");
        std::fs::write(dir.join("huge.md"), "x".repeat(MAX_CUSTOM_PROFILE_BYTES as usize + 1)).unwrap();
        assert!(find(&dir, "custom:huge").is_none());
    }

    #[test]
    fn missing_profile_falls_back_to_default() {
        let dir = temp_dir("missing");
        assert_eq!(resolve(&dir, "custom:gone").id, DEFAULT_PROFILE);
    }

    #[test]
    fn missing_folder_yields_only_builtins() {
        let dir = std::env::temp_dir().join("audiobooks-test-nonexistent-dir");
        assert_eq!(list(&dir).len(), 2);
    }
}
