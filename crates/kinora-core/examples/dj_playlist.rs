//! Try DJ sessions from playlists and mixes against the live YouTube API,
//! anonymously:
//!
//! ```sh
//! cargo run -p kinora-core --example dj_playlist -- [--shuffle] [--pages N] [PLAYLIST_ID_OR_URL ...]
//! ```
//!
//! Without ids it plays three public lists found with an anonymous search:
//! a long playlist (Billboard's Top 500 Pop Songs, ~250 tracks), a short one
//! (Daft Punk Greatest Hits, 50: it runs out and the radio takes over) and an
//! artist mix (Daft Punk, endless). For each it starts a session, fetches up
//! to N more pages (8), and prints what came from where and how long it
//! took, then the first page of the regular YouTube fallback alone.

use std::time::Instant;

use kinora_core::dj::{self, DjOptions, DjSeed, DjSession, RadioRequest};
use kinora_core::{AccountConfig, Core, Error, Paths, PlaylistKind};

const DEFAULTS: &[&str] = &[
    "PLQCXKl33aBijHlP4kjmQNP1TTeBlnKPoc",
    "PL5A-FXHBxgj3Gfluaje-MuhXC5iBsZF5w",
    "RDEMni3jl65KF37F9JYsJM8DGg",
];

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut shuffle = false;
    let mut pages = 8;
    let mut ids = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shuffle" => shuffle = true,
            "--pages" => pages = args.next().and_then(|p| p.parse().ok()).unwrap_or(pages),
            _ => ids.push(arg),
        }
    }
    if ids.is_empty() {
        ids = DEFAULTS.iter().map(|s| s.to_string()).collect();
    }

    let core = Core::new(Paths::new(std::env::temp_dir().join("kinora-dj-example")));
    let account = AccountConfig::anonymous();
    for id in ids {
        let Some(seed) = DjSeed::playlist(&id, None) else {
            println!("== {id:?} holds no playlist id");
            continue;
        };
        let seed = seed.shuffled(shuffle);
        let kind = PlaylistKind::of(seed.playlist_id().as_deref().unwrap_or_default());
        println!(
            "\n== {kind:?} {} (shuffle: {shuffle})",
            seed.playlist_id().unwrap()
        );

        let started = Instant::now();
        let mut session = match core.dj_start(&account, seed.clone()).await {
            Ok(session) => session,
            Err(e) => {
                println!("start failed: {e}");
                continue;
            }
        };
        println!(
            "start: {} queued from {:?} in {} ms",
            session.upcoming().len(),
            session.source(),
            started.elapsed().as_millis(),
        );
        more_pages(&core, &account, &mut session, pages).await;
        show(&mut session);

        let started = Instant::now();
        match dj::radio::youtube_mix(&seed).await {
            Ok(page) => println!(
                "fallback alone (regular YouTube): {} tracks, more: {}, in {} ms",
                page.tracks.len(),
                page.continuation.is_some(),
                started.elapsed().as_millis()
            ),
            Err(e) => println!("fallback alone failed: {e}"),
        }
    }
    Ok(())
}

/// Fetch up to `pages` more pages, stopping two pages after the playlist
/// gave way to a radio.
async fn more_pages(core: &Core, account: &AccountConfig, session: &mut DjSession, pages: usize) {
    let options = session.options();
    session.set_options(DjOptions {
        refill_below: usize::MAX,
        ..options
    });
    let mut radio_pages = 0;
    for _ in 0..pages {
        let Some(request) = session.refill_request() else {
            println!("nothing left to fetch");
            break;
        };
        let kind = match &request {
            RadioRequest::Start(seed) => format!("radio from {seed:?}"),
            RadioRequest::More(_) if session.in_playlist() => "playlist (or mix) page".to_owned(),
            RadioRequest::More(_) => "radio page".to_owned(),
        };
        let started = Instant::now();
        match core.dj_more(account, &request).await {
            Ok(page) => {
                let fetched = page.tracks.len();
                let ms = started.elapsed().as_millis();
                let source = page.source;
                let added = session.extend(&request, page);
                println!("{kind}: {fetched} fetched, {added} new, from {source:?}, in {ms} ms");
            }
            Err(e) => {
                println!("{kind} failed: {e}");
                session.fetch_failed(&request);
            }
        }
        if !session.in_playlist() {
            radio_pages += 1;
            if radio_pages >= 2 {
                break;
            }
        }
    }
    session.set_options(options);
}

fn show(session: &mut DjSession) {
    let from_playlist = session
        .upcoming()
        .iter()
        .filter(|t| session.is_from_playlist(t))
        .count();
    println!(
        "queue: {} tracks, {from_playlist} from the playlist, {} from radios",
        session.upcoming().len(),
        session.upcoming().len() - from_playlist,
    );
    session.advance();
    for (i, t) in session
        .current()
        .into_iter()
        .chain(session.upcoming().iter().take(5))
        .enumerate()
    {
        println!(
            "  {:>3}  {} {} - {}",
            i + 1,
            t.id,
            t.artists_line(),
            t.title
        );
    }
    // Where the playlist gives way to the radio.
    if let Some(at) = session
        .upcoming()
        .iter()
        .position(|t| !session.is_from_playlist(t))
        .filter(|&at| at > 0)
    {
        let (last, first) = (&session.upcoming()[at - 1], &session.upcoming()[at]);
        println!(
            "  ...  playlist ends with {} - {}, radio starts with {} - {}",
            last.artists_line(),
            last.title,
            first.artists_line(),
            first.title
        );
    }
}
