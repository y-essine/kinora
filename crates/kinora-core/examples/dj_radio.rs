//! Try the DJ engine against the live YouTube API, anonymously:
//!
//! ```sh
//! cargo run -p kinora-core --example dj_radio -- [VIDEO_ID | mymix] [PAGES]
//! ```
//!
//! Starts a session from the seed (Rick Astley's "Never Gonna Give You Up"
//! by default), fetches PAGES more pages (3), prints the queue, what dedupe
//! left out and the planned transitions, then does the same with the YouTube
//! fallback alone.

use std::time::{Duration, Instant};

use kinora_core::dj::{self, DjOptions, DjSeed, DjSession, RadioRequest, Track};
use kinora_core::{AccountConfig, Core, Error, Paths};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed = match args.first().map(String::as_str) {
        None => DjSeed::Video("dQw4w9WgXcQ".into()),
        Some("mymix") => DjSeed::MyMix,
        Some(id) => DjSeed::Video(id.to_owned()),
    };
    let pages: usize = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(3);

    let core = Core::new(Paths::new(std::env::temp_dir().join("kinora-dj-example")));
    let account = AccountConfig::anonymous();

    println!("== YouTube Music radio for {seed:?}");
    let started = Instant::now();
    let mut session = core.dj_start(&account, seed.clone()).await?;
    println!(
        "start: {} queued from {:?} in {} ms (personal: {})",
        session.upcoming().len(),
        session.source(),
        started.elapsed().as_millis(),
        session.personal(),
    );
    more_pages(&core, &account, &mut session, pages).await;
    show(&core, &mut session);

    println!("\n== YouTube fallback alone for {seed:?}");
    let started = Instant::now();
    let page = dj::radio::youtube_mix(&seed).await?;
    let fetched = page.tracks.len();
    let mut fallback = DjSession::start(seed, core.dj_settings().options(), page);
    println!(
        "start: {fetched} fetched, {} queued from {:?} in {} ms",
        fallback.upcoming().len(),
        fallback.source(),
        started.elapsed().as_millis(),
    );
    more_pages(&core, &account, &mut fallback, pages.min(2)).await;
    show(&core, &mut fallback);
    Ok(())
}

/// Fetch `pages` more pages, whatever the queue's length.
async fn more_pages(core: &Core, account: &AccountConfig, session: &mut DjSession, pages: usize) {
    let options = session.options();
    session.set_options(DjOptions {
        refill_below: usize::MAX,
        ..options
    });
    for _ in 0..pages {
        let Some(request) = session.refill_request() else {
            println!("nothing left to fetch");
            break;
        };
        let kind = match &request {
            RadioRequest::Start(seed) => format!("new radio from {seed:?}"),
            RadioRequest::More(_) => "continuation".to_owned(),
        };
        let started = Instant::now();
        match core.dj_more(account, &request).await {
            Ok(page) => {
                let fetched = page.tracks.len();
                let ms = started.elapsed().as_millis();
                let added = session.extend(&request, page);
                println!("{kind}: {fetched} fetched, {added} new, in {ms} ms");
            }
            Err(e) => {
                println!("{kind} failed: {e}");
                session.fetch_failed(&request);
            }
        }
    }
    session.set_options(options);
}

fn show(core: &Core, session: &mut DjSession) {
    println!("queue: {} tracks", session.upcoming().len());
    let line = |t: &Track| {
        let album = t
            .album
            .as_ref()
            .map_or(String::new(), |a| format!(" [{}]", a.name));
        let length = t.duration.map_or("live".to_owned(), |d| {
            format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60)
        });
        format!(
            "{} {:>5}  {} - {}{album} ({:?})",
            t.id,
            length,
            t.artists_line(),
            t.title,
            t.kind
        )
    };
    if let Some(current) = session.current() {
        println!("  now  {}", line(current));
    }
    for (i, track) in session.upcoming().iter().take(12).enumerate() {
        println!("  {:>3}  {}", i + 1, line(track));
    }

    // Play through a few, planning each transition 10 s into the track.
    if session.current().is_none() {
        session.advance();
    }
    for _ in 0..4 {
        let (Some(current), Some(next)) = (session.current(), session.peek_next()) else {
            break;
        };
        let plan = core.dj_transition(current.duration, Duration::from_secs(10), next);
        let mid = plan.gains(plan.fade / 2);
        println!(
            "  {} -> {}: start at {:?}, fade {:?}, halfway out {:.3} / in {:.3}",
            current.id, next.id, plan.start_at, plan.fade, mid.outgoing, mid.incoming
        );
        session.advance();
    }
}
