//! Try the beat analysis on real audio.
//!
//! ```sh
//! ffmpeg -i TRACK -ac 1 -ar 11025 -f f32le track.raw
//! cargo run --release -p kinora-core --example beats_file -- track.raw [WINDOW_SECONDS] [STEP_SECONDS]
//! ```
//!
//! Prints the grid of the track's first seconds (what the DJ analyses ahead
//! of time), then of a window that slides over the track (what it finds in
//! the tail of whatever plays), each with the time the analysis took. A
//! steady track should give the same tempo all along, and beats that stay
//! where the first grid puts them. Second line: where "Start on the Beat"
//! would bring the track in (`ENTRY_ONLY=1` stops there).

use std::time::Instant;

use kinora_core::beats::{ANALYSIS_RATE, ENTRY_WINDOW, INTRO, analyse, find_entry};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: beats_file TRACK.raw [WINDOW_SECONDS] [STEP_SECONDS]");
        std::process::exit(2);
    };
    let window: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(12.0);
    let step: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(20.0);
    let bytes = std::fs::read(&path).expect("a readable file");
    let pcm: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let rate = ANALYSIS_RATE;
    let seconds = pcm.len() as f64 / rate as f64;
    let cut = |from: f64, length: f64| {
        let a = ((from * rate as f64) as usize).min(pcm.len());
        let b = (((from + length) * rate as f64) as usize).min(pcm.len());
        &pcm[a..b]
    };

    let started = Instant::now();
    let intro = analyse(cut(0.0, INTRO), rate, 0.0, None);
    let took = started.elapsed();
    match intro {
        Some(grid) => println!(
            "intro ({INTRO:.0} s of {seconds:.0} s): {:.2} BPM, first beat {:.3} s, confidence {:.2}, in {:.1} ms",
            grid.bpm,
            grid.anchor,
            grid.confidence,
            took.as_secs_f64() * 1000.0
        ),
        None => println!("intro: no beat"),
    }

    // "Start on the Beat": where a crossfade would bring the track in. The
    // file's length stands for the track's (a cut file: say `LENGTH=240`).
    let length = std::env::var("LENGTH").ok().and_then(|l| l.parse().ok());
    let started = Instant::now();
    let entry = find_entry(cut(0.0, ENTRY_WINDOW), rate, length.or(Some(seconds)));
    println!(
        "entry: {:.2} s ({}{}), in {:.1} ms",
        entry.start,
        entry.found,
        entry.grid.map_or(String::new(), |g| format!(
            "; {:.2} BPM, confidence {:.2}",
            g.bpm, g.confidence
        )),
        started.elapsed().as_secs_f64() * 1000.0
    );
    if std::env::var("ENTRY_ONLY").is_ok() {
        return;
    }

    let mut from = 0.0;
    while from + window <= seconds {
        let started = Instant::now();
        let free = analyse(cut(from, window), rate, from, None);
        let took = started.elapsed();
        let near = intro.and_then(|grid| analyse(cut(from, window), rate, from, Some(grid.bpm)));
        let show = |grid: Option<kinora_core::beats::BeatGrid>| match grid {
            Some(grid) => {
                // Against the intro's grid, carried all the way here.
                let drift = intro.map_or(f64::NAN, |first| {
                    let phase = first.phase_at(grid.anchor);
                    (if phase > 0.5 { phase - 1.0 } else { phase }) * first.period() * 1000.0
                });
                format!(
                    "{:7.2} BPM conf {:.2} ({:+5.0} ms off the intro's grid)",
                    grid.bpm, grid.confidence, drift
                )
            }
            None => "no beat".to_owned(),
        };
        println!(
            "{from:6.1} s  free {}   near the intro's {}   {:.1} ms",
            show(free),
            show(near),
            took.as_secs_f64() * 1000.0
        );
        from += step;
    }
}
