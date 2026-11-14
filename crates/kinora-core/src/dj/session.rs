//! A DJ session: what plays now, what comes next, what already played.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::dedupe::track_key;
use super::radio::{DjSeed, RadioContinuation, RadioPage, RadioRequest, RadioSource};
use super::track::Track;

/// Ask for more tracks when fewer than this many are left.
pub const DEFAULT_REFILL_BELOW: usize = 10;
/// Pages in a row that add nothing new before the session stops following
/// the radio and starts a new one from a recent track.
const FRUITLESS_PAGES: u8 = 2;

/// How a session behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DjOptions {
    /// Never bring back a song that already played in this session (the
    /// "avoid repeats" setting). Off, only the queue itself stays free of
    /// duplicates.
    pub avoid_repeats: bool,
    /// [`DjSession::needs_refill`] when fewer upcoming tracks than this.
    pub refill_below: usize,
}

impl Default for DjOptions {
    fn default() -> Self {
        Self {
            avoid_repeats: true,
            refill_below: DEFAULT_REFILL_BELOW,
        }
    }
}

/// A track that played (or was skipped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub track: Track,
    /// Skipped by the user rather than played to its end.
    pub skipped: bool,
}

/// A continuous music session. A plain value with no I/O: the frontend keeps
/// it (on its UI thread, or behind a mutex), asks the [`Core`](crate::Core)
/// for pages ([`Self::refill_request`] → `Core::dj_more`) and feeds them in
/// with [`Self::extend`].
///
/// Duplicates never enter the queue: not the same video twice, and not the
/// same song by the same artist in another upload ([`super::same_song`]).
/// With [`DjOptions::avoid_repeats`], nothing that already played comes back
/// either. Tracks the user removed stay out for the whole session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DjSession {
    seed: DjSeed,
    options: DjOptions,
    source: RadioSource,
    personal: bool,
    current: Option<Track>,
    upcoming: Vec<Track>,
    history: Vec<HistoryEntry>,
    continuation: Option<RadioContinuation>,
    /// Ids and [`track_key`]s of everything that was current.
    played: HashSet<String>,
    /// Ids and keys of what the user removed.
    blocked: HashSet<String>,
    /// Videos whose radio this session already used.
    seeds: HashSet<String>,
    fruitless: u8,
}

impl DjSession {
    /// A session from the first page of its radio. For a video seed the
    /// video (the page's first track) becomes [`Self::current`]: it is
    /// already playing. For "my mix" nothing is current yet: call
    /// [`Self::advance`] for the first track.
    pub fn start(seed: DjSeed, options: DjOptions, page: RadioPage) -> Self {
        let mut session = Self {
            source: page.source,
            personal: page.personal,
            seed: seed.clone(),
            options,
            current: None,
            upcoming: Vec::new(),
            history: Vec::new(),
            continuation: None,
            played: HashSet::new(),
            blocked: HashSet::new(),
            seeds: HashSet::new(),
            fruitless: 0,
        };
        session.extend(&RadioRequest::Start(seed.clone()), page);
        if let DjSeed::Video(id) = &seed
            && session.upcoming.first().is_some_and(|t| &t.id == id)
        {
            let track = session.upcoming.remove(0);
            session.make_current(track);
        }
        session
    }

    pub fn seed(&self) -> &DjSeed {
        &self.seed
    }

    pub fn options(&self) -> DjOptions {
        self.options
    }

    /// Change the options (the user changed a setting mid-session).
    pub fn set_options(&mut self, options: DjOptions) {
        self.options = options;
    }

    /// Where the first page came from ([`RadioSource::YouTube`]: the
    /// YouTube Music radio failed and this is the fallback).
    pub fn source(&self) -> RadioSource {
        self.source
    }

    /// The radio is the account's (signed in).
    pub fn personal(&self) -> bool {
        self.personal
    }

    /// What plays now.
    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }

    /// What plays after it, in order.
    pub fn upcoming(&self) -> &[Track] {
        &self.upcoming
    }

    /// The next track, the one to prepare and to plan the transition to.
    pub fn peek_next(&self) -> Option<&Track> {
        self.upcoming.first()
    }

    /// What played, oldest first.
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// Nothing plays, nothing is queued, and there is no way to get more.
    pub fn is_finished(&self) -> bool {
        self.current.is_none() && self.upcoming.is_empty() && self.refill_request().is_none()
    }

    // ---- Moving on ----

    /// The current track played to its end: the next one becomes current
    /// (and is returned). `None`: the queue is empty (refill it).
    pub fn advance(&mut self) -> Option<&Track> {
        self.move_on(false)
    }

    /// The user skipped the current track: like [`Self::advance`], but the
    /// history says so.
    pub fn skip(&mut self) -> Option<&Track> {
        self.move_on(true)
    }

    fn move_on(&mut self, skipped: bool) -> Option<&Track> {
        if let Some(track) = self.current.take() {
            self.history.push(HistoryEntry { track, skipped });
        }
        if self.upcoming.is_empty() {
            return None;
        }
        let next = self.upcoming.remove(0);
        self.make_current(next);
        self.current.as_ref()
    }

    fn make_current(&mut self, track: Track) {
        self.played.insert(track.id.clone());
        self.played.insert(track_key(&track));
        self.current = Some(track);
    }

    // ---- Editing the queue ----

    /// Put `track` right after the current one ("play next"). The user asked
    /// for it, so it goes in even if it played before; if it is queued
    /// already, it moves up.
    pub fn play_next(&mut self, track: Track) {
        self.insert(0, track);
    }

    /// Put `track` at the end of the queue ("add to queue").
    pub fn enqueue(&mut self, track: Track) {
        self.insert(usize::MAX, track);
    }

    fn insert(&mut self, index: usize, track: Track) {
        self.upcoming.retain(|t| t.id != track.id);
        self.blocked.remove(&track.id);
        self.blocked.remove(&track_key(&track));
        let index = index.min(self.upcoming.len());
        self.upcoming.insert(index, track);
    }

    /// Take a track (by video id) out of the queue. It, and other uploads of
    /// the same song, do not come back in this session.
    pub fn remove(&mut self, id: &str) -> Option<Track> {
        let index = self.upcoming.iter().position(|t| t.id == id)?;
        let track = self.upcoming.remove(index);
        self.blocked.insert(track.id.clone());
        self.blocked.insert(track_key(&track));
        Some(track)
    }

    /// Move a queued track (by video id) to `index` in the queue (clamped).
    /// `false` if it is not queued.
    pub fn move_to(&mut self, id: &str, index: usize) -> bool {
        let Some(from) = self.upcoming.iter().position(|t| t.id == id) else {
            return false;
        };
        let track = self.upcoming.remove(from);
        let index = index.min(self.upcoming.len());
        self.upcoming.insert(index, track);
        true
    }

    // ---- Refilling ----

    /// Fewer than [`DjOptions::refill_below`] tracks are left.
    pub fn needs_refill(&self) -> bool {
        self.upcoming.len() < self.options.refill_below
    }

    /// What to fetch to top the queue up: the radio's next page, or, when it
    /// has none left, the radio of the most recent track whose radio was not
    /// used yet. `None` when the queue is full enough, or when there is
    /// nothing left to start from.
    pub fn refill_request(&self) -> Option<RadioRequest> {
        if !self.needs_refill() {
            return None;
        }
        if let Some(continuation) = &self.continuation {
            return Some(RadioRequest::More(continuation.clone()));
        }
        self.current
            .iter()
            .chain(self.history.iter().rev().map(|h| &h.track))
            .chain(self.upcoming.iter().rev())
            .find(|t| !self.seeds.contains(&t.id))
            .map(|t| RadioRequest::Start(DjSeed::Video(t.id.clone())))
    }

    /// Fetching `request` failed: don't ask for it again. A continuation is
    /// dropped (the next request starts a radio from a recent track); a
    /// track whose radio failed is not used as a seed again.
    pub fn fetch_failed(&mut self, request: &RadioRequest) {
        match request {
            RadioRequest::More(c) if self.continuation.as_ref() == Some(c) => {
                self.continuation = None;
            }
            RadioRequest::Start(DjSeed::Video(id)) => {
                self.seeds.insert(id.clone());
            }
            _ => {}
        }
    }

    /// Whether [`Self::extend`] would queue `track`.
    pub fn accepts(&self, track: &Track) -> bool {
        let key = track_key(track);
        let queued = self.current.iter().chain(&self.upcoming);
        let taken = |s: &String| {
            self.blocked.contains(s) || (self.options.avoid_repeats && self.played.contains(s))
        };
        !taken(&track.id)
            && !taken(&key)
            && !queued
                .into_iter()
                .any(|t| t.id == track.id || track_key(t) == key)
    }

    /// Add a fetched page (the answer to `request`) to the queue, leaving out
    /// duplicates. Returns how many tracks were added.
    pub fn extend(&mut self, request: &RadioRequest, page: RadioPage) -> usize {
        if let RadioRequest::Start(DjSeed::Video(id)) = request {
            self.seeds.insert(id.clone());
        }
        let mut added = 0;
        for track in page.tracks {
            if self.accepts(&track) {
                self.upcoming.push(track);
                added += 1;
            }
        }
        self.continuation = page.continuation;
        if added == 0 {
            self.fruitless += 1;
            if self.fruitless >= FRUITLESS_PAGES {
                // This radio only repeats itself: start another one.
                self.continuation = None;
                self.fruitless = 0;
            }
        } else {
            self.fruitless = 0;
        }
        added
    }
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
            duration: Some(std::time::Duration::from_secs(200)),
            thumbnail: None,
            year: None,
            kind: TrackKind::Song,
        }
    }

    fn page(tracks: Vec<Track>, continuation: Option<RadioContinuation>) -> RadioPage {
        RadioPage {
            tracks,
            continuation,
            source: RadioSource::Music,
            personal: false,
        }
    }

    fn ids(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|t| t.id.as_str()).collect()
    }

    /// A continuation, from a real parse so the type stays opaque.
    fn more() -> Option<RadioContinuation> {
        serde_json::from_value(
            serde_json::json!({ "music": { "playlist": "RDAMVMa", "token": "t" } }),
        )
        .ok()
    }

    fn session(avoid_repeats: bool) -> DjSession {
        let options = DjOptions {
            avoid_repeats,
            refill_below: 3,
        };
        let first = page(
            vec![
                track("seed", "Cheri Cheri Lady", "Modern Talking"),
                track("b", "Brother Louie", "Modern Talking"),
                track("c", "Cheri Cheri Lady (Live)", "Modern Talking"),
                track("d", "Take On Me", "a-ha"),
                track("b", "Brother Louie", "Modern Talking"),
            ],
            more(),
        );
        DjSession::start(DjSeed::Video("seed".into()), options, first)
    }

    #[test]
    fn a_video_seed_is_current_and_duplicates_stay_out() {
        let s = session(true);
        assert_eq!(s.current().unwrap().id, "seed");
        // The live version of the seed and the repeated "b" are left out.
        assert_eq!(ids(s.upcoming()), ["b", "d"]);
        assert_eq!(s.peek_next().unwrap().id, "b");
        assert!(s.needs_refill());
        assert!(matches!(s.refill_request(), Some(RadioRequest::More(_))));
    }

    #[test]
    fn advancing_and_skipping_fill_the_history() {
        let mut s = session(true);
        assert_eq!(s.advance().unwrap().id, "b");
        assert_eq!(s.skip().unwrap().id, "d");
        assert_eq!(s.advance(), None);
        assert_eq!(s.current(), None);
        let history: Vec<_> = s
            .history()
            .iter()
            .map(|h| (h.track.id.as_str(), h.skipped))
            .collect();
        assert_eq!(history, [("seed", false), ("b", true), ("d", false)]);
    }

    #[test]
    fn played_songs_do_not_come_back_unless_repeats_are_allowed() {
        let later = || {
            page(
                vec![
                    track(
                        "seed2",
                        "Cheri Cheri Lady (Official Video)",
                        "Modern Talking",
                    ),
                    track("b", "Brother Louie", "Modern Talking"),
                    track("e", "Venus", "Bananarama"),
                ],
                more(),
            )
        };
        let request = RadioRequest::More(more().unwrap());

        let mut avoid = session(true);
        avoid.advance(); // "b" plays
        assert_eq!(avoid.extend(&request, later()), 1);
        assert_eq!(ids(avoid.upcoming()), ["d", "e"]);

        let mut repeat = session(false);
        repeat.advance(); // "b" plays, "seed" is history
        // "b" is current, so it still stays out; the seed may come back.
        assert_eq!(repeat.extend(&request, later()), 2);
        assert_eq!(ids(repeat.upcoming()), ["d", "seed2", "e"]);
    }

    #[test]
    fn the_user_edits_the_queue() {
        let mut s = session(true);
        // "Play next" goes first, even a song that played already.
        s.play_next(track("x", "Venus", "Bananarama"));
        s.play_next(track("seed", "Cheri Cheri Lady", "Modern Talking"));
        assert_eq!(ids(s.upcoming()), ["seed", "x", "b", "d"]);
        // Inserting a queued track moves it.
        s.play_next(track("d", "Take On Me", "a-ha"));
        assert_eq!(ids(s.upcoming()), ["d", "seed", "x", "b"]);
        s.enqueue(track("y", "Africa", "TOTO"));
        assert_eq!(ids(s.upcoming()).last(), Some(&"y"));
        assert!(s.move_to("y", 0));
        assert!(!s.move_to("nope", 0));
        assert_eq!(s.upcoming()[0].id, "y");

        // Removed tracks, and other uploads of them, stay out.
        assert_eq!(s.remove("x").unwrap().title, "Venus");
        assert_eq!(s.remove("x"), None);
        let request = RadioRequest::More(more().unwrap());
        let again = page(
            vec![
                track("x", "Venus", "Bananarama"),
                track("x2", "Venus (Official Video)", "Bananarama"),
                track("z", "Down Under", "Men At Work"),
            ],
            None,
        );
        assert_eq!(s.extend(&request, again), 1);
        assert!(!s.accepts(&track("x3", "Venus [HD]", "Bananarama")));
        // Unless the user asks for them.
        s.play_next(track("x", "Venus", "Bananarama"));
        assert_eq!(s.upcoming()[0].id, "x");
    }

    #[test]
    fn a_finished_radio_starts_another_from_a_recent_track() {
        let mut s = session(true);
        let request = RadioRequest::More(more().unwrap());
        s.extend(
            &request,
            page(vec![track("e", "Venus", "Bananarama")], None),
        );
        s.advance(); // b
        // The seed's radio is used up: the most recent track ("b") seeds the next.
        assert_eq!(
            s.refill_request(),
            Some(RadioRequest::Start(DjSeed::Video("b".into())))
        );
        let reseed = RadioRequest::Start(DjSeed::Video("b".into()));
        s.extend(
            &reseed,
            page(vec![track("b", "Brother Louie", "Modern Talking")], None),
        );
        // "b" is used too: the one before it was the seed, so "e" (queued) is next.
        assert_eq!(
            s.refill_request(),
            Some(RadioRequest::Start(DjSeed::Video("e".into())))
        );
    }

    #[test]
    fn a_radio_that_only_repeats_itself_is_dropped() {
        let mut s = session(true);
        let request = RadioRequest::More(more().unwrap());
        let same = || page(vec![track("b", "Brother Louie", "Modern Talking")], more());
        assert_eq!(s.extend(&request, same()), 0);
        assert!(matches!(s.refill_request(), Some(RadioRequest::More(_))));
        assert_eq!(s.extend(&request, same()), 0);
        assert!(matches!(s.refill_request(), Some(RadioRequest::Start(_))));
    }

    #[test]
    fn failed_fetches_are_not_repeated() {
        let mut s = session(true);
        let Some(more @ RadioRequest::More(_)) = s.refill_request() else {
            panic!("the radio continues");
        };
        s.fetch_failed(&more);
        let start = s.refill_request().unwrap();
        // The seed's radio was used: the newest queued track is next.
        assert_eq!(start, RadioRequest::Start(DjSeed::Video("d".into())));
        s.fetch_failed(&start);
        assert_eq!(
            s.refill_request(),
            Some(RadioRequest::Start(DjSeed::Video("b".into())))
        );
    }

    #[test]
    fn my_mix_starts_with_nothing_playing() {
        let first = page(vec![track("a", "A", "X"), track("b", "B", "Y")], None);
        let mut s = DjSession::start(DjSeed::MyMix, DjOptions::default(), first);
        assert_eq!(s.current(), None);
        assert_eq!(s.advance().unwrap().id, "a");
        assert!(!s.is_finished());
        // Nothing to refill from but the queue.
        assert_eq!(
            s.refill_request(),
            Some(RadioRequest::Start(DjSeed::Video("a".into())))
        );
    }
}
