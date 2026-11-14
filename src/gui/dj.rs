//! Kinora: the DJ (see `docs/design.md`, "DJ", and `docs/architecture.md`,
//! "The DJ"). The engine is `kinora_core::dj`; this is its frontend.
//!
//! The DJ is a mode of the one player, not another place: every track is an
//! ordinary video page in the player sheet, with the DJ's queue where Up next
//! would be. [`Dj`] keeps the session on the UI thread and runs its loop:
//!
//! - while a track plays, the next one is prepared and the transition is
//!   planned from the player's real length;
//! - shortly before the planned start, the next track's page is opened *over*
//!   the current one, which keeps playing without its picture. Once the new
//!   player really plays, both volumes follow the plan's equal-power curve
//!   every 50 ms; then the old player is paused and let go;
//! - a cut (short or live tracks, crossfade off) waits for the end of stream;
//! - the queue is topped up in the background.
//!
//! Why this is not a `kinora_core::player::Player` adapter with
//! `crossfade_to`: a track here is a whole page (its own Clapper player with
//! prepared-stream fallbacks, title, details), and the crossfade is between
//! two pages. An adapter over one `clapper::Player` could not own that.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::subclass::types::ObjectSubclassIsExt;
use gtk::{gdk, glib};
use kinora_core::dj::{DjSeed, DjSession, RadioSource, Track, TransitionPlan};

use crate::backend::{Client, Video};
use crate::gspawn;
use crate::gui::video_page::VideoPage;
use crate::gui::window::Window;

/// How often the loop looks at the player.
const TICK: Duration = Duration::from_millis(200);
/// How often the volumes are set during a fade.
const FADE_STEP: Duration = Duration::from_millis(50);
/// The next player is opened this long before the planned start: about what
/// a prepared stream needs to begin playing.
const LEAD: Duration = Duration::from_millis(2000);
/// A skipped track does not wait for the next one: it fades out at once.
const SKIP_FADE: Duration = Duration::from_millis(450);
/// Give up waiting for the incoming player after this long.
const START_TIMEOUT: Duration = Duration::from_secs(40);
/// After a failed request for more tracks, wait before asking again.
const REFILL_PAUSE: Duration = Duration::from_secs(15);
/// How many queued tracks the panel lists.
const SHOWN: usize = 30;

/// A fade that is running.
struct Fade {
    /// The page that is ending. It is no longer in the window.
    outgoing: VideoPage,
    /// The page that fades in. `None`: a skip, only the old one fades out.
    incoming: Option<VideoPage>,
    /// How long the fade should take.
    wanted: Duration,
    began: Instant,
    /// Set once the incoming player plays: the curve, and when it started.
    running: Option<(TransitionPlan, Instant)>,
    steps: u32,
    timer: glib::SourceId,
}

struct Inner {
    window: glib::WeakRef<Window>,
    session: RefCell<Option<DjSession>>,
    panel: Panel,
    /// The page of the current track.
    page: RefCell<Option<VideoPage>>,
    fade: RefCell<Option<Fade>>,
    tick: RefCell<Option<glib::SourceId>>,
    /// The id of the track that was prepared ahead.
    prepared: RefCell<Option<String>>,
    starting: Cell<bool>,
    refilling: Cell<bool>,
    refill_after: Cell<Option<Instant>>,
    /// Counts sessions, so answers for an old one are dropped.
    generation: Cell<u32>,
}

/// The DJ of one window.
#[derive(Clone)]
pub struct Dj(Rc<Inner>);

impl Dj {
    pub fn new(window: &Window) -> Self {
        let dj = Self(Rc::new(Inner {
            window: window.downgrade(),
            session: RefCell::default(),
            panel: Panel::new(),
            page: RefCell::default(),
            fade: RefCell::default(),
            tick: RefCell::default(),
            prepared: RefCell::default(),
            starting: Cell::default(),
            refilling: Cell::default(),
            refill_after: Cell::default(),
            generation: Cell::default(),
        }));
        let weak = Rc::downgrade(&dj.0);
        dj.0.panel.connect(move |action| {
            if let Some(inner) = weak.upgrade() {
                Dj(inner).panel_action(action);
            }
        });
        dj
    }

    pub fn is_on(&self) -> bool {
        self.0.session.borrow().is_some()
    }

    fn client(&self) -> Option<Client> {
        self.0.window.upgrade().map(|w| w.client())
    }

    fn toast(&self, text: &str) {
        if let Some(window) = self.0.window.upgrade() {
            window.show_toast(adw::Toast::new(text));
        }
    }

    /// Start the DJ: from the video in the player (following the "start
    /// from" setting), or with `from_mix` from the account's mix.
    pub fn start(&self, from_mix: bool) {
        self.start_from(from_mix.then_some(DjSeed::MyMix));
    }

    /// Play a playlist or a mix with the DJ: all of it, in order, each item
    /// fading into the next.
    pub fn start_playlist(&self, id: &str, title: &str) {
        match playlist_seed(id, title) {
            Some(seed) => self.start_from(Some(seed)),
            None => {
                log::info!("DJ: no playlist seed in the engine yet ({id}, {title})");
                self.toast(&gettextrs::gettext("The DJ cannot play playlists yet"));
            }
        }
    }

    /// Start the DJ from `seed` (the mix, a playlist, ...); `None`: from the
    /// video in the player, following the "start from" setting.
    pub fn start_from(&self, seed: Option<DjSeed>) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        if self.0.starting.get() {
            return;
        }
        if self.is_on() {
            // Asked for something in particular: that replaces what plays.
            if seed.is_none() {
                window.imp().expand_player();
                return;
            }
            self.leave();
        }
        let client = window.client();
        if !client.dj_enabled() {
            self.toast(&gettextrs::gettext(
                "The DJ is switched off. Turn it on in Preferences, Extensions",
            ));
            return;
        }
        let video = window
            .imp()
            .current_video_page()
            .and_then(|p| p.video())
            .filter(|v| v.platform() == "youtube");
        let seed = seed.unwrap_or_else(|| client.dj_seed(video.as_ref()));
        log::info!("DJ: starting from {seed:?}");
        self.0.starting.set(true);
        let dj = self.clone();
        let generation = self.0.generation.get();
        gspawn!(async move {
            let started = Instant::now();
            let result = client.dj_start(seed.clone()).await;
            dj.0.starting.set(false);
            if dj.0.generation.get() != generation || dj.is_on() {
                return;
            }
            match result {
                Ok(session) => {
                    log::info!(
                        "DJ: {} tracks from {:?} in {:.1} s (personal: {})",
                        session.upcoming().len(),
                        session.source(),
                        started.elapsed().as_secs_f64(),
                        session.personal()
                    );
                    dj.begin(session, seed);
                }
                Err(e) => {
                    log::warn!("DJ: could not start: {e}");
                    dj.toast(&gettextrs::gettext("The DJ could not find music to play"));
                }
            }
        });
    }

    /// A session arrived: take over the playing video, or open the first track.
    fn begin(&self, mut session: DjSession, seed: DjSeed) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        // A video's radio takes over the video that plays; every other seed
        // (the mix, a playlist) starts with its own first track.
        let taken_over = seed_video(&seed).and_then(|id| {
            window
                .imp()
                .current_video_page()
                .and_then(|p| p.video().map(|v| (p, v)))
                .filter(|(_, v)| v.id().id == id)
        });
        if let Some(id) = seed_video(&seed) {
            let Some((page, video)) = taken_over else {
                log::info!("DJ: the video is gone; not starting");
                return;
            };
            if session.current().is_none_or(|t| t.id != id) {
                session.play_next(Track::from_video(&video.video()));
                session.advance();
            }
            self.0.session.replace(Some(session));
            page.set_dj(true);
            self.adopt(&page);
        } else {
            let first = match session.current() {
                Some(track) => Some(track.clone()),
                None => session.advance().cloned(),
            };
            let Some(track) = first else {
                self.toast(&gettextrs::gettext("The DJ could not find music to play"));
                return;
            };
            self.0.session.replace(Some(session));
            let video = window.client().dj_video(&track);
            let (page, _) = window.imp().open_dj_track(&video, false);
            self.adopt(&page);
        }
        window.set_dj_on(true);
        let dj = Rc::downgrade(&self.0);
        let tick = glib::timeout_add_local(TICK, move || match dj.upgrade() {
            Some(inner) => {
                Dj(inner).tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        if let Some(old) = self.0.tick.replace(Some(tick)) {
            old.remove();
        }
        self.refill();
    }

    /// Leave the DJ: what plays goes on as an ordinary video, with Up next
    /// and autoplay as before.
    pub fn leave(&self) {
        let inner = &self.0;
        inner.generation.set(inner.generation.get().wrapping_add(1));
        inner.starting.set(false);
        if inner.session.take().is_none() {
            return;
        }
        log::info!("DJ: off");
        self.finish_fade();
        if let Some(tick) = inner.tick.take() {
            tick.remove();
        }
        inner.prepared.take();
        inner.refilling.set(false);
        inner.refill_after.set(None);
        if let Some(page) = inner.page.take() {
            page.set_dj(false);
        }
        inner.panel.clear();
        if let Some(window) = inner.window.upgrade() {
            window.set_dj_on(false);
        }
    }

    /// The user skips the current track.
    pub fn skip(&self) {
        if !self.is_on() {
            return;
        }
        self.finish_fade();
        self.move_on(true);
    }

    /// `page` is the current track's page from now on.
    fn adopt(&self, page: &VideoPage) {
        self.0.page.replace(Some(page.clone()));
        self.0.prepared.take();
        page.set_dj_panel(Some(self.0.panel.widget()));
        self.refresh_panel();
    }

    fn refresh_panel(&self) {
        let (Some(client), session) = (self.client(), self.0.session.borrow()) else {
            return;
        };
        if let Some(session) = session.as_ref() {
            self.0.panel.show(session, &client);
        }
    }

    // ---- The loop ----

    fn tick(&self) {
        let inner = &self.0;
        let (Some(window), Some(page)) = (inner.window.upgrade(), inner.page.borrow().clone())
        else {
            return;
        };
        // The player shows something else, or is gone: the DJ is over.
        if window.imp().current_video_page().as_ref() != Some(&page) {
            return self.leave();
        }
        if inner.fade.borrow().is_some() {
            return;
        }
        let client = window.client();
        let state = page.player_state();
        let playing = state == clapper::PlayerState::Playing;
        let position = page.position_secs();
        let duration = page.duration_secs();
        let next = inner
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.peek_next().cloned());

        // Get the next track ready once this one is under way.
        if let Some(next) = &next
            && playing
            && position > 3.0
            && inner.prepared.borrow().as_deref() != Some(next.id.as_str())
        {
            log::info!("DJ: preparing {} ({})", next.id, next.title);
            inner.prepared.replace(Some(next.id.clone()));
            client.prepare_playback(&client.dj_video(next));
        }
        self.refill();

        // The track ended by itself (a cut, or nothing was ready to fade to).
        let ended = matches!(
            state,
            clapper::PlayerState::Stopped | clapper::PlayerState::Paused
        ) && duration.is_some_and(|d| position >= d - 0.75);
        if ended {
            self.move_on(false);
            return;
        }

        if let (true, Some(duration), Some(next)) = (playing, duration, &next) {
            let at = Duration::from_secs_f64(position.max(0.0));
            let plan = client.dj_transition(Some(Duration::from_secs_f64(duration)), at, next);
            if let Some(start) = plan.start_at
                && !plan.is_cut()
                && at + LEAD >= start
            {
                self.crossfade(plan.fade);
            }
        }
    }

    /// Make the next track current and open its page in place of the current
    /// one. `skipped`: the user asked; the old track fades out quickly.
    fn move_on(&self, skipped: bool) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let next = inner.session.borrow_mut().as_mut().and_then(|s| {
            if skipped {
                s.skip().cloned()
            } else {
                s.advance().cloned()
            }
        });
        let Some(track) = next else {
            // Nothing queued: more may be on its way; else the DJ is done.
            let finished = inner
                .session
                .borrow()
                .as_ref()
                .is_none_or(|s| s.refill_request().is_none());
            if finished && !inner.refilling.get() {
                self.toast(&gettextrs::gettext("The DJ has run out of music"));
                self.leave();
            }
            return;
        };
        log::info!(
            "DJ: {} {} ({})",
            if skipped { "skip to" } else { "cut to" },
            track.id,
            track.title
        );
        let video = window.client().dj_video(&track);
        let (page, old) = window.imp().open_dj_track(&video, skipped);
        self.adopt(&page);
        if let Some(outgoing) = old {
            self.start_fade(outgoing, None, SKIP_FADE);
        }
    }

    /// Open the next track's page over the current one and fade between them.
    fn crossfade(&self, fade: Duration) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let next = inner
            .session
            .borrow_mut()
            .as_mut()
            .and_then(|s| s.advance().cloned());
        let Some(track) = next else {
            return;
        };
        log::info!(
            "DJ: crossfade to {} ({}), planned over {:.1} s",
            track.id,
            track.title,
            fade.as_secs_f64()
        );
        let video = window.client().dj_video(&track);
        let (page, old) = window.imp().open_dj_track(&video, true);
        self.adopt(&page);
        match old {
            Some(outgoing) => {
                // Silent until it really plays; then the curve takes over.
                page.set_fade_gain(Some(0.0));
                self.start_fade(outgoing, Some(page), fade);
            }
            None => page.set_fade_gain(None),
        }
    }

    fn start_fade(&self, outgoing: VideoPage, incoming: Option<VideoPage>, wanted: Duration) {
        let dj = Rc::downgrade(&self.0);
        let timer = glib::timeout_add_local(FADE_STEP, move || match dj.upgrade() {
            Some(inner) => Dj(inner).fade_step(),
            None => glib::ControlFlow::Break,
        });
        let running = incoming
            .is_none()
            .then(|| (fade_plan(wanted), Instant::now()));
        self.0.fade.replace(Some(Fade {
            outgoing,
            incoming,
            wanted,
            began: Instant::now(),
            running,
            steps: 0,
            timer,
        }));
    }

    fn fade_step(&self) -> glib::ControlFlow {
        let mut slot = self.0.fade.borrow_mut();
        let Some(fade) = slot.as_mut() else {
            return glib::ControlFlow::Break;
        };
        if fade.running.is_none() {
            let started = fade
                .incoming
                .as_ref()
                .is_none_or(|p| p.player_state() == clapper::PlayerState::Playing);
            if started {
                // Fade over what the old track has left, at most.
                let out = &fade.outgoing;
                let left = match (out.player_state(), out.duration_secs()) {
                    (clapper::PlayerState::Playing, Some(d)) => {
                        Duration::from_secs_f64((d - out.position_secs()).max(0.0))
                    }
                    _ => Duration::ZERO,
                };
                let over = fade.wanted.min(left);
                log::info!(
                    "DJ: fading over {:.1} s ({:.1} s after opening the next player)",
                    over.as_secs_f64(),
                    fade.began.elapsed().as_secs_f64()
                );
                fade.running = Some((fade_plan(over), Instant::now()));
            } else if fade.began.elapsed() > START_TIMEOUT {
                log::warn!("DJ: the next track did not start; giving up the fade");
                drop(slot);
                self.finish_fade();
                return glib::ControlFlow::Break;
            } else {
                return glib::ControlFlow::Continue;
            }
        }
        let Some((plan, since)) = fade.running else {
            return glib::ControlFlow::Continue;
        };
        let elapsed = since.elapsed();
        if plan.is_cut() || elapsed >= plan.fade {
            drop(slot);
            // The source is removed by returning Break, not by `finish_fade`.
            self.end_fade(false);
            return glib::ControlFlow::Break;
        }
        let gains = plan.gains(elapsed);
        fade.outgoing.set_fade_gain(Some(gains.outgoing));
        if let Some(incoming) = &fade.incoming {
            incoming.set_fade_gain(Some(gains.incoming));
        }
        // Twice a second is enough to follow it in the log.
        if fade.steps % 10 == 0 {
            log::info!(
                "DJ: fade {:.2}/{:.2} s  out {:.2}  in {:.2}",
                elapsed.as_secs_f64(),
                plan.fade.as_secs_f64(),
                gains.outgoing,
                if fade.incoming.is_some() {
                    gains.incoming
                } else {
                    1.0
                }
            );
        }
        fade.steps += 1;
        glib::ControlFlow::Continue
    }

    /// End a running fade now: the new track at full volume, the old one
    /// paused and let go.
    fn finish_fade(&self) {
        self.end_fade(true);
    }

    fn end_fade(&self, remove_timer: bool) {
        let Some(fade) = self.0.fade.take() else {
            return;
        };
        if remove_timer {
            fade.timer.remove();
        }
        if let Some(incoming) = &fade.incoming {
            incoming.set_fade_gain(None);
        }
        log::info!("DJ: fade done; releasing the old player");
        // Pause only, and drop it a moment later: stopping a Clapper player,
        // or ending one while another starts, has crashed (VideoPage::stop).
        let outgoing = fade.outgoing;
        outgoing.set_fade_gain(Some(0.0));
        outgoing.stop();
        glib::timeout_add_local_once(Duration::from_millis(1500), move || drop(outgoing));
    }

    // ---- The queue ----

    /// Top the queue up in the background when it runs low.
    fn refill(&self) {
        let inner = &self.0;
        if inner.refilling.get() || inner.refill_after.get().is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(client) = self.client() else {
            return;
        };
        let request = {
            let mut session = inner.session.borrow_mut();
            let Some(session) = session.as_mut() else {
                return;
            };
            // The settings may have changed since the session began.
            session.set_options(kinora_core::dj::DjOptions {
                refill_below: session.options().refill_below,
                ..client.core().dj_settings().options()
            });
            session.refill_request()
        };
        let Some(request) = request else {
            return;
        };
        inner.refilling.set(true);
        let dj = self.clone();
        let generation = inner.generation.get();
        gspawn!(async move {
            let result = client.dj_more(request.clone()).await;
            if dj.0.generation.get() != generation {
                return;
            }
            dj.0.refilling.set(false);
            if let Some(session) = dj.0.session.borrow_mut().as_mut() {
                match result {
                    Ok(page) => {
                        let fetched = page.tracks.len();
                        let added = session.extend(&request, page);
                        log::info!(
                            "DJ: refill, {added} of {fetched} new, {} queued",
                            session.upcoming().len()
                        );
                    }
                    Err(e) => {
                        log::warn!("DJ: refill failed: {e}");
                        session.fetch_failed(&request);
                        dj.0.refill_after.set(Some(Instant::now() + REFILL_PAUSE));
                    }
                }
            }
            dj.refresh_panel();
        });
    }

    fn panel_action(&self, action: PanelAction) {
        match action {
            PanelAction::Skip => return self.skip(),
            PanelAction::Leave => return self.leave(),
            PanelAction::PlayNow(id) => {
                if let Some(session) = self.0.session.borrow_mut().as_mut() {
                    session.move_to(&id, 0);
                }
                return self.skip();
            }
            PanelAction::PlayNext(id) => self.edit(|s| {
                s.move_to(&id, 0);
            }),
            PanelAction::Remove(id) => self.edit(|s| {
                s.remove(&id);
            }),
            PanelAction::Move(id, index) => self.edit(|s| {
                s.move_to(&id, index);
            }),
        }
        self.refresh_panel();
    }

    fn edit(&self, change: impl FnOnce(&mut DjSession)) {
        if let Some(session) = self.0.session.borrow_mut().as_mut() {
            change(session);
        }
    }

    /// Test hook: what the session holds, for the log.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_log(&self) {
        match self.0.session.borrow().as_ref() {
            Some(s) => log::info!(
                "DEV: DJ on, now {:?}, next {:?}, {} queued, {} played, source {:?}",
                s.current().map(|t| &t.title),
                s.upcoming()
                    .iter()
                    .take(3)
                    .map(|t| &t.title)
                    .collect::<Vec<_>>(),
                s.upcoming().len(),
                s.history().len(),
                s.source()
            ),
            None => log::info!("DEV: DJ off"),
        }
    }

    /// Test hook: act on the queue as the panel's buttons would. `what` is
    /// `play-next`, `remove` or `play-now`, applied to the queue's third track.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_queue(&self, what: &str) {
        let id = self
            .0
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.upcoming().get(2).map(|t| t.id.clone()));
        let Some(id) = id else { return };
        self.panel_action(match what {
            "play-next" => PanelAction::PlayNext(id),
            "remove" => PanelAction::Remove(id),
            _ => PanelAction::PlayNow(id),
        });
        self.dev_log();
    }
}

/// The seed that plays a playlist, an album or a mix (by id or link).
fn playlist_seed(id: &str, title: &str) -> Option<DjSeed> {
    DjSeed::playlist(id, Some(title.to_owned()).filter(|t| !t.is_empty()))
}

/// The video a seed takes over (it is already playing), if it is one.
fn seed_video(seed: &DjSeed) -> Option<&str> {
    match seed {
        DjSeed::Video(id) => Some(id),
        _ => None,
    }
}

/// A fade of `over` that starts now (a cut when there is nothing to fade).
fn fade_plan(over: Duration) -> TransitionPlan {
    if over < Duration::from_millis(100) {
        TransitionPlan::CUT
    } else {
        TransitionPlan {
            start_at: Some(Duration::ZERO),
            fade: over,
        }
    }
}

/// "3:32", or "1:02:05".
fn format_length(length: Option<Duration>) -> String {
    let Some(length) = length else {
        return String::new();
    };
    let s = length.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "Album · 2019" for a song; nothing for a plain video.
fn album_line(track: &Track) -> String {
    match (&track.album, track.year) {
        (Some(album), Some(year)) => format!("{} · {year}", album.name),
        (Some(album), None) => album.name.clone(),
        (None, Some(year)) => year.to_string(),
        (None, None) => String::new(),
    }
}

// ---- The panel ------------------------------------------------------------

enum PanelAction {
    Skip,
    Leave,
    PlayNow(String),
    PlayNext(String),
    Remove(String),
    Move(String, usize),
}

type Handler = Rc<RefCell<Option<Box<dyn Fn(PanelAction)>>>>;

/// The DJ's part of the video page: what plays (artwork, artists, album) and
/// the queue. One panel moves from page to page with the music.
struct Panel {
    root: gtk::Box,
    note: gtk::Label,
    art: gtk::Picture,
    title: gtk::Label,
    artists: gtk::Label,
    album: gtk::Label,
    count: gtk::Label,
    list: gtk::ListBox,
    more: gtk::Label,
    /// What is shown, to leave the widgets alone when nothing changed.
    shown_now: RefCell<Option<String>>,
    shown_queue: RefCell<Vec<String>>,
    handler: Handler,
}

/// A square picture of `size` that never asks for more room than that.
fn artwork(size: i32) -> (gtk::Widget, gtk::Picture) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .build();
    let overlay = gtk::Overlay::builder()
        .child(&frame)
        .overflow(gtk::Overflow::Hidden)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["k-dj-art"])
        .build();
    overlay.add_overlay(&picture);
    (overlay.upcast(), picture)
}

/// Show `video`'s thumbnail in `picture`, loading it if need be.
fn show_thumbnail(picture: &gtk::Picture, video: &Video, client: &Client) {
    let set = |picture: &gtk::Picture, video: &Video| {
        let texture = video.thumbnail().map(|p| gdk::Texture::for_pixbuf(&p));
        picture.set_paintable(texture.as_ref());
    };
    set(picture, video);
    if video.thumbnail().is_some() {
        return;
    }
    let (picture, video, client) = (picture.downgrade(), video.clone(), client.clone());
    gspawn!(async move {
        client.populate_thumbnail(&video).await;
        if let Some(picture) = picture.upgrade() {
            set(&picture, &video);
        }
    });
}

fn label(css: &[&str], lines: i32) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(css.to_vec())
        .build();
    if lines > 1 {
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(lines);
    }
    label
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build()
}

impl Panel {
    fn new() -> Self {
        let handler: Handler = Rc::default();
        let emit = |handler: &Handler, action: PanelAction| {
            if let Some(f) = handler.borrow().as_ref() {
                f(action);
            }
        };

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .css_classes(["k-dj"])
            .build();

        // "DJ on", where the music comes from, and the way out.
        let header = gtk::Box::builder().spacing(8).build();
        let on = gtk::Label::builder()
            .label(gettextrs::gettext("DJ on"))
            .css_classes(["k-kicker", "k-dj-on"])
            .valign(gtk::Align::Center)
            .build();
        let note = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["k-dj-note"])
            .build();
        let leave = gtk::Button::builder()
            .label(gettextrs::gettext("Leave"))
            .tooltip_text(gettextrs::gettext(
                "Leave the DJ: this video plays on, with Up next as usual",
            ))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "k-dj-leave"])
            .build();
        leave.connect_clicked(glib::clone!(
            #[strong]
            handler,
            move |_| emit(&handler, PanelAction::Leave)
        ));
        header.append(&on);
        header.append(&note);
        header.append(&leave);

        // What plays.
        let now = gtk::Box::builder()
            .spacing(12)
            .css_classes(["k-dj-now"])
            .build();
        let (art_frame, art) = artwork(64);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let title = label(&["k-dj-title"], 2);
        let artists = label(&["k-card-meta"], 1);
        let album = label(&["k-dj-album"], 1);
        text.append(&title);
        text.append(&artists);
        text.append(&album);
        let skip = icon_button(
            "media-skip-forward-symbolic",
            &gettextrs::gettext("Skip (Shift+N)"),
        );
        skip.connect_clicked(glib::clone!(
            #[strong]
            handler,
            move |_| emit(&handler, PanelAction::Skip)
        ));
        now.append(&art_frame);
        now.append(&text);
        now.append(&skip);

        // The queue.
        let queue_header = gtk::Box::builder().spacing(8).build();
        let heading = gtk::Label::builder()
            .label(gettextrs::gettext("Up next"))
            .xalign(0.0)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Heading)
            .css_classes(["k-row-title"])
            .build();
        let count = gtk::Label::builder()
            .css_classes(["dim-label", "numeric"])
            .build();
        queue_header.append(&heading);
        queue_header.append(&count);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .css_classes(["k-upnext-list", "k-dj-queue"])
            .build();
        list.connect_row_activated(glib::clone!(
            #[strong]
            handler,
            move |_, row| emit(&handler, PanelAction::PlayNow(row.widget_name().into()))
        ));
        let more = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["k-dj-note"])
            .margin_start(8)
            .build();
        let queue = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        queue.append(&queue_header);
        queue.append(&list);
        queue.append(&more);

        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .css_classes(["k-panel", "k-dj-card"])
            .build();
        top.append(&header);
        top.append(&now);
        root.append(&top);
        root.append(&queue);

        Self {
            root,
            note,
            art,
            title,
            artists,
            album,
            count,
            list,
            more,
            shown_now: RefCell::default(),
            shown_queue: RefCell::default(),
            handler,
        }
    }

    fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    fn connect(&self, f: impl Fn(PanelAction) + 'static) {
        self.handler.replace(Some(Box::new(f)));
    }

    /// Take the panel out of its page and forget what it showed.
    fn clear(&self) {
        if let Some(parent) = self.root.parent().and_downcast::<gtk::Box>() {
            parent.remove(&self.root);
        }
        self.shown_now.take();
        self.shown_queue.take();
        self.list.remove_all();
    }

    fn show(&self, session: &DjSession, client: &Client) {
        self.note
            .set_label(&match (session.source(), session.personal()) {
                (RadioSource::YouTube, _) => gettextrs::gettext("From YouTube"),
                (RadioSource::Music, true) => gettextrs::gettext("Picked for you"),
                (RadioSource::Music, false) => String::new(),
            });

        if let Some(track) = session.current()
            && self.shown_now.borrow().as_deref() != Some(track.id.as_str())
        {
            self.shown_now.replace(Some(track.id.clone()));
            self.title.set_label(&track.title);
            self.artists.set_label(&track.artists_line());
            let album = album_line(track);
            self.album.set_label(&album);
            self.album.set_visible(!album.is_empty());
            show_thumbnail(&self.art, &client.dj_video(track), client);
        }

        let upcoming = session.upcoming();
        self.count.set_label(&upcoming.len().to_string());
        let hidden = upcoming.len().saturating_sub(SHOWN);
        self.more.set_visible(hidden > 0);
        if hidden > 0 {
            self.more
                .set_label(&gettextrs::gettext("and {} more").replace("{}", &hidden.to_string()));
        }
        let ids: Vec<String> = upcoming.iter().take(SHOWN).map(|t| t.id.clone()).collect();
        if *self.shown_queue.borrow() == ids {
            return;
        }
        self.list.remove_all();
        for (index, track) in upcoming.iter().take(SHOWN).enumerate() {
            self.list.append(&self.row(track, index, client));
        }
        self.shown_queue.replace(ids);
    }

    /// One queued track: artwork, title, artists, length, and on hover or
    /// focus "play next" and "remove". Rows can be dragged to reorder.
    fn row(&self, track: &Track, index: usize, client: &Client) -> gtk::ListBoxRow {
        let handler = &self.handler;
        let emit = |handler: &Handler, action: PanelAction| {
            if let Some(f) = handler.borrow().as_ref() {
                f(action);
            }
        };
        let line = gtk::Box::builder().spacing(10).build();
        let (art_frame, art) = artwork(40);
        show_thumbnail(&art, &client.dj_video(track), client);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let title = label(&["k-dj-row-title"], 1);
        title.set_label(&track.title);
        let artists = label(&["k-card-meta"], 1);
        artists.set_label(&track.artists_line());
        text.append(&title);
        text.append(&artists);
        let length = gtk::Label::builder()
            .label(format_length(track.duration))
            .valign(gtk::Align::Center)
            .css_classes(["k-card-meta", "numeric", "k-dj-length"])
            .build();
        let actions = gtk::Box::builder()
            .css_classes(["k-dj-row-actions"])
            .build();
        let play_next = icon_button("go-top-symbolic", &gettextrs::gettext("Play Next"));
        // The first one is next already; its button keeps its place, unseen.
        if index == 0 {
            play_next.set_opacity(0.0);
            play_next.set_sensitive(false);
        }
        let remove = icon_button(
            "window-close-symbolic",
            &gettextrs::gettext("Remove from Queue"),
        );
        let id = track.id.clone();
        play_next.connect_clicked(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_| emit(&handler, PanelAction::PlayNext(id.clone()))
        ));
        remove.connect_clicked(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_| emit(&handler, PanelAction::Remove(id.clone()))
        ));
        actions.append(&play_next);
        actions.append(&remove);
        line.append(&art_frame);
        line.append(&text);
        line.append(&length);
        line.append(&actions);

        let row = gtk::ListBoxRow::builder()
            .child(&line)
            .name(track.id.as_str())
            .tooltip_text(gettextrs::gettext("Play now, or drag to reorder"))
            .build();
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}, {}",
            track.title,
            track.artists_line()
        ))]);

        // Reorder by dragging a row onto another: it takes that one's place.
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        drag.connect_prepare(glib::clone!(
            #[strong]
            id,
            move |_, _, _| Some(gdk::ContentProvider::for_value(&id.to_value()))
        ));
        drag.connect_drag_begin(|source, _| {
            if let Some(row) = source.widget() {
                source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row))), 24, 24);
            }
        });
        row.add_controller(drag);
        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        drop.connect_drop(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_, value, _, _| match value.get::<String>() {
                Ok(dragged) if dragged != id => {
                    emit(&handler, PanelAction::Move(dragged, index));
                    true
                }
                _ => false,
            }
        ));
        row.add_controller(drop);
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_like_a_clock() {
        assert_eq!(format_length(Some(Duration::from_secs(212))), "3:32");
        assert_eq!(format_length(Some(Duration::from_secs(3725))), "1:02:05");
        assert_eq!(format_length(None), "");
    }

    #[test]
    fn nothing_to_fade_is_a_cut() {
        assert!(fade_plan(Duration::ZERO).is_cut());
        let plan = fade_plan(Duration::from_secs(6));
        assert!(!plan.is_cut());
        // Equal power: both at about 0.71 halfway.
        let half = plan.gains(Duration::from_secs(3));
        assert!((half.outgoing - half.incoming).abs() < 1e-9);
        assert!((half.outgoing - kinora_core::dj::equal_power(0.5).outgoing).abs() < 1e-9);
    }
}
