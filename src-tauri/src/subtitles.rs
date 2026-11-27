//! English subtitles (SRT) for the exported video, built from the text each narration was voiced from. A scene's
//! cues share its narration time in proportion to their length, so times inside a scene are approximate.

/// A cue is at most two lines on screen.
const MAX_CUE_CHARS: usize = 84;
const LINE_CHARS: usize = 42;

/// One scene's narration on the video timeline.
#[derive(Debug, Clone, Copy)]
pub struct Narration<'a> {
    /// The text the audio was generated from, voice direction tags included.
    pub text: &'a str,
    pub start: f64,
    pub secs: f64,
}

/// Removes `[softly]`-style voice directions (YouTube would show them) and tidies the spaces.
fn strip_tags(text: &str) -> String {
    let mut clean = String::new();
    let mut rest = text;
    while let Some((before, tagged)) = rest.split_once('[') {
        let Some((_, after)) = tagged.split_once(']') else {
            break;
        };
        clean.push_str(before);
        clean.push(' ');
        rest = after;
    }
    clean.push_str(rest);
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Splits after every word that `ends` a group.
fn split_after(text: &str, ends: impl Fn(&str) -> bool) -> Vec<String> {
    let mut groups = Vec::new();
    let mut group: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        group.push(word);
        if ends(word) {
            groups.push(group.join(" "));
            group.clear();
        }
    }
    if !group.is_empty() {
        groups.push(group.join(" "));
    }
    groups
}

fn ends_sentence(word: &str) -> bool {
    word.trim_end_matches(['"', '\'', '”', '’', ')', '»'])
        .ends_with(['.', '!', '?', '…'])
}

/// Joins pieces with spaces while the result still fits in one cue.
fn pack(pieces: impl Iterator<Item = String>) -> Vec<String> {
    let mut packed: Vec<String> = Vec::new();
    for piece in pieces {
        match packed.last_mut() {
            Some(last) if chars(last) + 1 + chars(&piece) <= MAX_CUE_CHARS => {
                last.push(' ');
                last.push_str(&piece);
            }
            _ => packed.push(piece),
        }
    }
    packed
}

/// One cue per sentence; a long one breaks at commas where possible, else between words.
fn cues(text: &str) -> Vec<String> {
    split_after(&strip_tags(text), ends_sentence)
        .iter()
        .flat_map(|sentence| {
            let clauses = split_after(sentence, |word| word.ends_with([',', ';', ':']));
            pack(clauses.into_iter().flat_map(|clause| {
                if chars(&clause) <= MAX_CUE_CHARS {
                    vec![clause]
                } else {
                    pack(clause.split_whitespace().map(str::to_owned))
                }
            }))
        })
        .collect()
}

/// Breaks a long cue into two lines at the space nearest its middle.
fn wrap(cue: &str) -> String {
    let length = chars(cue);
    if length <= LINE_CHARS {
        return cue.to_owned();
    }
    cue.char_indices()
        .enumerate()
        .filter(|(_, (_, c))| *c == ' ')
        .min_by_key(|(index, _)| (2 * index).abs_diff(length))
        .map_or_else(
            || cue.to_owned(),
            |(_, (byte, _))| format!("{}\n{}", &cue[..byte], &cue[byte + 1..]),
        )
}

fn stamp(secs: f64) -> String {
    let millis = (secs * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}

/// The SRT file's content; empty when there is nothing to show.
pub fn srt(narrations: &[Narration]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for narration in narrations {
        let cues = cues(narration.text);
        let total: usize = cues.iter().map(|cue| chars(cue)).sum();
        let mut at = narration.start;
        for cue in cues {
            let end = at + narration.secs * chars(&cue) as f64 / total as f64;
            blocks.push(format!(
                "{}\n{} --> {}\n{}\n",
                blocks.len() + 1,
                stamp(at),
                stamp(end),
                wrap(&cue)
            ));
            at = end;
        }
    }
    blocks.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tags_removes_directions_and_keeps_an_unclosed_bracket() {
        assert_eq!(
            strip_tags("[softly] The fox slept. [long pause]\n\n[deep voice] Goodnight."),
            "The fox slept. Goodnight."
        );
        assert_eq!(strip_tags("a [ b"), "a [ b");
        assert_eq!(strip_tags("[pause]"), "");
    }

    #[test]
    fn each_sentence_is_its_own_cue_including_quoted_ones() {
        assert_eq!(
            cues("One. \"Two!\" “Three?” Four"),
            ["One.", "\"Two!\"", "“Three?”", "Four"]
        );
    }

    #[test]
    fn a_long_sentence_breaks_at_commas_first_and_then_between_words() {
        let first = "The little fox curled up in the soft green moss beside the stream,";
        let second = "and the moon rose slowly over the quiet hills.";
        assert_eq!(cues(&format!("{first} {second}")), [first, second]);

        let endless = "word ".repeat(40);
        let parts = cues(&endless);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|cue| cue.chars().count() <= MAX_CUE_CHARS));
        assert_eq!(parts.join(" "), endless.trim());
    }

    #[test]
    fn wrap_splits_near_the_middle_and_counts_characters_not_bytes() {
        assert_eq!(wrap("Short line."), "Short line.");
        assert_eq!(
            wrap("The little fox curled up in the soft green moss."),
            "The little fox curled up\nin the soft green moss."
        );
        let accented = "é".repeat(LINE_CHARS);
        assert_eq!(wrap(&accented), accented);
        let unbroken = "a".repeat(LINE_CHARS + 1);
        assert_eq!(wrap(&unbroken), unbroken);
    }

    #[test]
    fn stamp_is_the_srt_time_format() {
        assert_eq!(stamp(0.0), "00:00:00,000");
        assert_eq!(stamp(3723.4567), "01:02:03,457");
    }

    #[test]
    fn cues_share_the_narration_time_by_length_and_are_numbered_across_scenes() {
        let out = srt(&[
            Narration {
                text: "[softly] Abc. Abcdefg.",
                start: 0.5,
                secs: 6.0,
            },
            Narration {
                text: "[pause]",
                start: 7.5,
                secs: 1.0,
            },
            Narration {
                text: "End.",
                start: 9.5,
                secs: 2.0,
            },
        ]);
        assert_eq!(
            out,
            "1\n00:00:00,500 --> 00:00:02,500\nAbc.\n\n\
             2\n00:00:02,500 --> 00:00:06,500\nAbcdefg.\n\n\
             3\n00:00:09,500 --> 00:00:11,500\nEnd.\n"
        );
        assert_eq!(srt(&[]), "");
    }
}
