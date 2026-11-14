//! Telling songs apart: the same song by the same artist often comes back as
//! another upload ("(Official Video)", "(Lyrics)", "(2022 Remaster)", "Live",
//! the "Artist - Title" upload of a channel). A DJ session plays it once.

use super::track::Track;

/// Words in brackets (or after " - ") that make a different recording worth
/// hearing again: a remix is not a repeat, a remaster is.
const KEEP_WORDS: &[&str] = &[
    "remix",
    "mix",
    "cover",
    "acoustic",
    "instrumental",
    "karaoke",
    "nightcore",
    "slowed",
    "sped",
    "reverb",
    "mashup",
    "bootleg",
    "rework",
    "vip",
];

/// Words that mark a " - ..." suffix as a version note ("Song - Remastered
/// 2011", "Song - Live at Wembley"), not part of the title.
const VERSION_WORDS: &[&str] = &[
    "remaster",
    "remastered",
    "live",
    "edit",
    "version",
    "official",
    "video",
    "audio",
    "lyrics",
    "lyric",
    "mono",
    "stereo",
    "hd",
    "4k",
    "visualizer",
    "visualiser",
];

/// The key two tracks share when they are the same song by the same artist:
/// the main artist and the title, without version notes, brackets, "feat."
/// and punctuation. Not meant for display.
pub fn track_key(track: &Track) -> String {
    let artist = track.artist();
    let title = split_artist_title(&track.title, artist).map_or(track.title.as_str(), |(_, t)| t);
    format!("{}|{}", artist_key(artist), title_key(title))
}

/// Whether two tracks are the same song (same video, or near-duplicates).
pub fn same_song(a: &Track, b: &Track) -> bool {
    a.id == b.id || track_key(a) == track_key(b)
}

/// "Modern Talking - Cheri Cheri Lady" from the channel "Modern Talking
/// Offiziell": `("Modern Talking", "Cheri Cheri Lady")`. `None` unless the
/// part before the dash is (the start of) the channel's name, or the other
/// way round.
pub(crate) fn split_artist_title<'a>(title: &'a str, channel: &str) -> Option<(&'a str, &'a str)> {
    let (artist, rest) = [" - ", " – ", " — "]
        .iter()
        .find_map(|dash| title.split_once(dash))?;
    let (a, c) = (squash(artist), squash(channel));
    let related = a.chars().count() >= 2
        && c.chars().count() >= 2
        && (c.starts_with(&a) || a.starts_with(&c));
    let rest = rest.trim();
    (related && !rest.is_empty()).then(|| (artist.trim(), rest))
}

/// The main artist of "A, B & C", "A feat. B", "A x B": A.
fn artist_key(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let main = [", ", " & ", " feat", " ft.", " x ", " and ", " with "]
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .map_or(lower.as_str(), |i| &lower[..i]);
    squash(main)
}

fn title_key(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut kept = String::new();
    // Brackets go unless they hold a keep word: "(Official Video)",
    // "[4K]", "(feat. X)", "(Radio Edit)" go; "(Moreno J Remix)" stays.
    let mut depth = 0usize;
    let mut inner = String::new();
    for c in lower.chars() {
        match c {
            '(' | '[' | '{' | '【' => {
                if depth == 0 {
                    inner.clear();
                } else {
                    inner.push(c);
                }
                depth += 1;
            }
            ')' | ']' | '}' | '】' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if has_word(&inner, KEEP_WORDS) {
                        kept.push(' ');
                        kept.push_str(&inner);
                    }
                    kept.push(' ');
                } else {
                    inner.push(c);
                }
            }
            _ if depth > 0 => inner.push(c),
            _ => kept.push(c),
        }
    }

    // "Song feat. X", "Song ft. X".
    let mut title = kept.as_str();
    for sep in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(i) = title.find(sep) {
            title = &title[..i];
        }
    }
    // "Song - Remastered 2011", "Song | Official Video".
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((head, tail)) = title.split_once(sep)
            && has_word(tail, VERSION_WORDS)
            && !has_word(tail, KEEP_WORDS)
        {
            title = head;
        }
    }

    let key = squash(title);
    if key.is_empty() { squash(&lower) } else { key }
}

/// Whether `text` holds one of `words` as a whole word (or a word ending in
/// it: "remixed", no; "megamix", yes).
fn has_word(text: &str, words: &[&str]) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .any(|w| {
            words
                .iter()
                .any(|k| w == *k || (k.len() > 2 && w.ends_with(k)))
        })
}

/// Lowercase letters and digits only (any script): "a-ha" and "A Ha" are "aha".
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dj::track::{Artist, TrackKind};

    fn track(id: &str, title: &str, artist: &str) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artists: vec![Artist {
                name: artist.into(),
                channel_id: None,
            }],
            album: None,
            duration: None,
            thumbnail: None,
            year: None,
            kind: TrackKind::Song,
        }
    }

    fn same(a: (&str, &str), b: (&str, &str)) -> bool {
        same_song(&track("a", a.0, a.1), &track("b", b.0, b.1))
    }

    #[test]
    fn versions_of_a_song_are_the_same_song() {
        let lady = ("Cheri Cheri Lady", "Modern Talking");
        assert!(same(lady, ("Cheri Cheri Lady (Live)", "Modern Talking")));
        assert!(same(
            lady,
            ("Cheri Cheri Lady (Official Video)", "Modern Talking")
        ));
        assert!(same(lady, ("Cheri Cheri Lady [Lyrics]", "Modern Talking")));
        assert!(same(
            lady,
            (
                "Modern Talking - Cheri Cheri Lady (Official Video)",
                "Modern Talking"
            )
        ));
        assert!(same(
            ("Together Forever (2022 Remaster)", "Rick Astley"),
            ("Together Forever (Lyrics)", "Rick Astley")
        ));
        assert!(same(
            (
                "Get Lucky (Radio Edit - feat. Pharrell Williams and Nile Rodgers)",
                "Daft Punk"
            ),
            ("Get Lucky", "Daft Punk, Pharrell Williams & Nile Rodgers")
        ));
        assert!(same(
            ("Heroes - 2017 Remaster", "David Bowie"),
            ("Heroes | Official Video", "David Bowie")
        ));
        assert!(same(("Song feat. Someone", "A"), ("Song", "A & Someone")));
        assert!(same(
            ("Take On Me", "a-ha"),
            ("Take On Me (Official Video) [4K]", "A-HA")
        ));
    }

    #[test]
    fn different_songs_stay_apart() {
        assert!(!same(
            ("Words (Moreno J Remix)", "F.R. David"),
            ("Words", "F.R. David")
        ));
        assert!(!same(("Believe", "Cher"), ("Believe", "Imagine Dragons")));
        assert!(!same(("Africa", "TOTO"), ("Rosanna", "TOTO")));
        assert!(!same(("Song (Acoustic)", "A"), ("Song", "A")));
        // A title that is only brackets still has a key.
        assert!(!same(("(Intro)", "A"), ("(Outro)", "A")));
        // Other scripts are kept.
        assert!(!same(("كيفك ع فراقي", "A"), ("جيبولي خبرها", "A")));
        assert!(same(
            ("كيفك ع فراقي", "A"),
            ("كيفك ع فراقي (Official Video)", "A")
        ));
    }

    #[test]
    fn channel_prefixes_are_split_off() {
        assert_eq!(
            split_artist_title(
                "Modern Talking - Cheri Cheri Lady (Official Video)",
                "Modern Talking Offiziell"
            ),
            Some(("Modern Talking", "Cheri Cheri Lady (Official Video)"))
        );
        assert_eq!(
            split_artist_title("Survivor - Eye Of The Tiger", "Survivor Band"),
            Some(("Survivor", "Eye Of The Tiger"))
        );
        // A dash that is part of the title stays.
        assert_eq!(
            split_artist_title("Love - Live in Paris", "Some Channel"),
            None
        );
        assert_eq!(split_artist_title("No dash here", "No"), None);
    }
}
