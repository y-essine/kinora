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
//! - Next and Previous are the same crossfade, asked for by hand: it starts as
//!   soon as the other track plays, over the crossfade setting;
//! - the queue is topped up in the background.
//!
//! The DJ is audio first: unless "Show video" is on, a track is prepared and
//! played as audio only, and its page shows the artwork. To make changing
//! track immediate, the next track (and the queue row the pointer rests on)
//! is *warmed*: its page is built ahead of time and its player loaded and
//! held paused at the start, at most [`MAX_WARM`] of them.
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
/// "Previous" within this many seconds of a track's start goes back one
/// track; later it starts the track over.
const RESTART_AFTER: f64 = 3.0;
/// Give up waiting for the incoming player after this long.
const START_TIMEOUT: Duration = Duration::from_secs(40);
/// After a failed request for more tracks, wait before asking again.
const REFILL_PAUSE: Duration = Duration::from_secs(15);
/// With the next player warm, it starts at once: open it this long ahead.
const WARM_LEAD: Duration = Duration::from_millis(300);
/// How many players are kept loaded ahead: the next track, and the one the
/// pointer rests on in the queue. (Preparing is capped at two at a time too.)
const MAX_WARM: usize = 2;
/// The pointer rests this long on a queue row before the row is got ready.
const HOVER_DELAY: Duration = Duration::from_millis(400);
/// How many queued tracks the panel lists.
const SHOWN: usize = 30;

/// A fade that is running.
struct Fade {
    /// The page that is ending. It is no longer in the window.
    outgoing: VideoPage,
    /// The page that fades in.
    incoming: VideoPage,
    /// How long the fade should take.
    wanted: Duration,
    began: Instant,
    /// Set once the incoming player plays: the curve, and when it started.
    running: Option<(TransitionPlan, Instant)>,
    steps: u32,
    timer: glib::SourceId,
    /// How ready the incoming track was: "warm", "prepared" or "cold".
    readiness: &'static str,
}

/// A track loaded ahead of time: its page, with the player paused at the start.
struct Warm {
    id: String,
    page: VideoPage,
    began: Instant,
    /// Loaded (and said so in the log).
    ready: bool,
}

struct Inner {
    window: glib::WeakRef<Window>,
    session: RefCell<Option<DjSession>>,
    panel: Panel,
    /// The page of the current track.
    page: RefCell<Option<VideoPage>>,
    fade: RefCell<Option<Fade>>,
    tick: RefCell<Option<glib::SourceId>>,
    /// The ids of the tracks whose streams were asked for ahead of time.
    prepared: RefCell<Vec<String>>,
    /// Players loaded ahead (see [`Warm`]), oldest first.
    warm: RefCell<Vec<Warm>>,
    /// The queued track the pointer rested on last.
    hovered: RefCell<Option<String>>,
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
            warm: RefCell::default(),
            hovered: RefCell::default(),
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
            let (page, _) = window.imp().open_dj_track(&video, false, None);
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
        self.forget_ahead();
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

    /// Next: crossfade into the next track now.
    pub fn skip(&self) {
        self.step(false);
    }

    /// Previous: start the track over, or, within its first seconds, crossfade
    /// back into the one that played before.
    pub fn previous(&self) {
        let page = self.0.page.borrow().clone();
        match page {
            Some(page) if page.position_secs() > RESTART_AFTER || !self.can_go_back() => {
                log::info!("DJ: previous: starting the track over");
                page.restart();
            }
            _ => self.step(true),
        }
    }

    /// A video was picked somewhere in the app while the DJ is on. If it is
    /// in the queue (a song of the playlist or mix that plays), the DJ mixes
    /// into it like Next does, and `true` is returned; else it is not the
    /// DJ's to play.
    pub fn play_queued(&self, video: &Video) -> bool {
        let id = video.id().id;
        let queued = self
            .0
            .session
            .borrow_mut()
            .as_mut()
            .is_some_and(|s| s.upcoming().iter().any(|t| t.id == id) && s.move_to(&id, 0));
        if queued {
            log::info!("DJ: picked {id} from the queue");
            self.step(false);
            self.refresh_panel();
        }
        queued
    }

    /// Test hook: back one track, wherever the current one is.
    #[cfg(feature = "dev-hooks")]
    pub fn back(&self) {
        self.step(true);
    }

    /// There is a track to go on to.
    pub fn can_go_next(&self) -> bool {
        self.0
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.peek_next().is_some())
    }

    /// A track played before this one.
    pub fn can_go_back(&self) -> bool {
        self.0
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.can_go_back())
    }

    /// A track was asked for and has not started playing yet: the fade waits
    /// for it, and so do Next and Previous.
    pub fn is_pending(&self) -> bool {
        self.0
            .fade
            .borrow()
            .as_ref()
            .is_some_and(|f| f.running.is_none())
    }

    /// Next or Previous by hand: the other track's page opens over the
    /// current one, and they crossfade over the setting as soon as it plays.
    fn step(&self, back: bool) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        if !self.is_on() || self.is_pending() {
            return;
        }
        // A fade that is under way ends here: one crossfade at a time.
        self.finish_fade();
        let track = inner.session.borrow_mut().as_mut().and_then(|s| {
            if back {
                s.back().cloned()
            } else {
                s.skip().cloned()
            }
        });
        let Some(track) = track else {
            if !back {
                self.out_of_music();
            }
            return;
        };
        let client = window.client();
        let wanted = client.core().dj_settings().crossfade;
        log::info!(
            "DJ: {} by hand: crossfade to {} ({}) over the {:.0} s setting",
            if back { "previous" } else { "next" },
            track.id,
            track.title,
            wanted.as_secs_f64()
        );
        let (warmed, readiness) = self.take_ahead(&client, &track);
        let (page, old) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), true, warmed);
        self.adopt(&page);
        if let Some(outgoing) = old {
            // Silent until it really plays; then the curve takes over.
            page.set_fade_gain(Some(0.0));
            self.start_fade(outgoing, page, wanted, readiness);
        } else {
            page.set_fade_gain(None);
        }
    }

    /// Nothing is queued: more may be on its way; else the DJ is done.
    fn out_of_music(&self) {
        let inner = &self.0;
        let finished = inner
            .session
            .borrow()
            .as_ref()
            .is_none_or(|s| s.refill_request().is_none());
        if finished && !inner.refilling.get() {
            self.toast(&gettextrs::gettext("The DJ has run out of music"));
            self.leave();
        }
    }

    /// Tell the panel and the window's Next and Previous what is possible now.
    fn changed(&self) {
        self.0.panel.set_pending(self.is_pending());
        if let Some(window) = self.0.window.upgrade() {
            window.imp().update_transport();
        }
    }

    /// `page` is the current track's page from now on.
    fn adopt(&self, page: &VideoPage) {
        self.0.page.replace(Some(page.clone()));
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
        drop(session);
        self.changed();
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
        self.warm_up(&client, next.as_ref().filter(|_| playing && position > 3.0));
        self.refill();

        // The track ended by itself (a cut, or nothing was ready to fade to).
        let ended = matches!(
            state,
            clapper::PlayerState::Stopped | clapper::PlayerState::Paused
        ) && duration.is_some_and(|d| position >= d - 0.75);
        if ended {
            self.move_on();
            return;
        }

        if let (true, Some(duration), Some(next)) = (playing, duration, &next) {
            let at = Duration::from_secs_f64(position.max(0.0));
            let plan = client.dj_transition(Some(Duration::from_secs_f64(duration)), at, next);
            let lead = if self.is_warm(&next.id) {
                WARM_LEAD
            } else {
                LEAD
            };
            if let Some(start) = plan.start_at
                && !plan.is_cut()
                && at + lead >= start
            {
                self.crossfade(plan.fade);
            }
        }
    }

    /// The track ended by itself: the next one takes its place (a cut).
    fn move_on(&self) {
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
            return self.out_of_music();
        };
        log::info!("DJ: cut to {} ({})", track.id, track.title);
        let client = window.client();
        let (warmed, readiness) = self.take_ahead(&client, &track);
        log::info!("DJ: {} starts {readiness}", track.id);
        let (page, _) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), false, warmed);
        page.set_fade_gain(None);
        self.adopt(&page);
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
        let client = window.client();
        let (warmed, readiness) = self.take_ahead(&client, &track);
        let (page, old) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), true, warmed);
        self.adopt(&page);
        match old {
            Some(outgoing) => {
                // Silent until it really plays; then the curve takes over.
                page.set_fade_gain(Some(0.0));
                self.start_fade(outgoing, page, fade, readiness);
            }
            None => page.set_fade_gain(None),
        }
    }

    fn start_fade(
        &self,
        outgoing: VideoPage,
        incoming: VideoPage,
        wanted: Duration,
        readiness: &'static str,
    ) {
        let dj = Rc::downgrade(&self.0);
        let timer = glib::timeout_add_local(FADE_STEP, move || match dj.upgrade() {
            Some(inner) => Dj(inner).fade_step(),
            None => glib::ControlFlow::Break,
        });
        self.0.fade.replace(Some(Fade {
            outgoing,
            incoming,
            wanted,
            began: Instant::now(),
            running: None,
            steps: 0,
            timer,
            readiness,
        }));
        self.changed();
    }

    fn fade_step(&self) -> glib::ControlFlow {
        let mut slot = self.0.fade.borrow_mut();
        let Some(fade) = slot.as_mut() else {
            return glib::ControlFlow::Break;
        };
        if fade.running.is_none() {
            let started = fade.incoming.player_state() == clapper::PlayerState::Playing;
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
                    "DJ: started after {:.1} s ({}); fading over {:.1} s",
                    fade.began.elapsed().as_secs_f64(),
                    fade.readiness,
                    over.as_secs_f64()
                );
                fade.running = Some((fade_plan(over), Instant::now()));
                drop(slot);
                // No longer pending: Next and Previous are back.
                self.changed();
                return glib::ControlFlow::Continue;
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
        fade.incoming.set_fade_gain(Some(gains.incoming));
        // Twice a second is enough to follow it in the log.
        if fade.steps % 10 == 0 {
            log::info!(
                "DJ: fade {:.2}/{:.2} s  out {:.2}  in {:.2}",
                elapsed.as_secs_f64(),
                plan.fade.as_secs_f64(),
                gains.outgoing,
                gains.incoming
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
        fade.incoming.set_fade_gain(None);
        log::info!("DJ: fade done; releasing the old player");
        fade.outgoing.set_fade_gain(Some(0.0));
        release(fade.outgoing);
        self.changed();
    }

    // ---- Getting tracks ready ahead of time ----

    fn audio_only(client: &Client) -> bool {
        !client.core().dj_settings().show_video
    }

    /// The track's stream has been looked up (audio or video, as the DJ plays).
    fn is_prepared(client: &Client, video: &Video) -> bool {
        if Self::audio_only(client) {
            client.prepared_audio_playback_uri(video).is_some()
        } else {
            client.prepared_playback_uri(video).is_some()
        }
    }

    fn is_warm(&self, id: &str) -> bool {
        self.0.warm.borrow().iter().any(|w| w.id == id && w.ready)
    }

    /// Prepare the next track (`next`, once the current one is under way)
    /// and the queue row the pointer rests on, then load their players and
    /// hold them paused at the start. Called from the loop.
    fn warm_up(&self, client: &Client, next: Option<&Track>) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let hovered = inner.hovered.borrow().clone().and_then(|id| {
            let session = inner.session.borrow();
            session
                .as_ref()?
                .upcoming()
                .iter()
                .find(|t| t.id == id)
                .cloned()
        });
        let mut wanted: Vec<Track> = next.cloned().into_iter().collect();
        if let Some(hovered) = hovered.filter(|h| wanted.iter().all(|t| t.id != h.id)) {
            wanted.push(hovered);
        }

        // What is no longer queued is let go; what finished loading says so.
        let queued: Vec<String> = inner
            .session
            .borrow()
            .as_ref()
            .map(|s| s.upcoming().iter().map(|t| t.id.clone()).collect())
            .unwrap_or_default();
        let mut warm = inner.warm.borrow_mut();
        let (keep, gone): (Vec<Warm>, Vec<Warm>) =
            warm.drain(..).partition(|w| queued.contains(&w.id));
        *warm = keep;
        gone.into_iter().for_each(|w| release(w.page));
        for w in warm.iter_mut().filter(|w| !w.ready) {
            if w.page.is_warm() {
                w.ready = true;
                log::info!(
                    "DJ: warm {} ready in {:.1} s",
                    w.id,
                    w.began.elapsed().as_secs_f64()
                );
            }
        }

        for track in &wanted {
            if warm.iter().any(|w| w.id == track.id) {
                continue;
            }
            let video = client.dj_video(track);
            if !Self::is_prepared(client, &video) {
                let mut asked = inner.prepared.borrow_mut();
                if !asked.contains(&track.id) {
                    log::info!("DJ: preparing {} ({})", track.id, track.title);
                    asked.push(track.id.clone());
                    if Self::audio_only(client) {
                        client.prepare_audio_playback(&video);
                    } else {
                        client.prepare_playback(&video);
                    }
                }
                continue;
            }
            // Room for it: the least recent one nobody waits for goes.
            if warm.len() >= MAX_WARM {
                let Some(index) = warm
                    .iter()
                    .position(|w| wanted.iter().all(|t| t.id != w.id))
                else {
                    continue;
                };
                release(warm.remove(index).page);
            }
            log::info!("DJ: warming {} ({})", track.id, track.title);
            let page = window.imp().new_dj_page(&video);
            page.warm();
            warm.push(Warm {
                id: track.id.clone(),
                page,
                began: Instant::now(),
                ready: false,
            });
        }
    }

    /// The track is about to play: its warmed page, if there is one, and how
    /// ready it is (for the log).
    fn take_ahead(&self, client: &Client, track: &Track) -> (Option<VideoPage>, &'static str) {
        let mut warm = self.0.warm.borrow_mut();
        match warm.iter().position(|w| w.id == track.id) {
            Some(index) => {
                let w = warm.remove(index);
                let readiness = if w.ready || w.page.is_warm() {
                    "warm"
                } else {
                    "warming"
                };
                (Some(w.page), readiness)
            }
            None if Self::is_prepared(client, &client.dj_video(track)) => (None, "prepared"),
            None => (None, "cold"),
        }
    }

    /// Let go of everything that was got ready (the DJ is left, or changes
    /// between audio and video).
    fn forget_ahead(&self) {
        self.0.prepared.borrow_mut().clear();
        self.0.hovered.take();
        let warm: Vec<Warm> = self.0.warm.borrow_mut().drain(..).collect();
        warm.into_iter().for_each(|w| release(w.page));
    }

    /// The panel's "Show video" was switched.
    fn set_show_video(&self, show: bool) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        if window.client().core().dj_settings().show_video == show {
            return;
        }
        log::info!("DJ: {}", if show { "with video" } else { "audio only" });
        crate::gui::extension_settings::store(
            &window.imp().settings,
            kinora_core::extension::builtin::dj::ID,
            kinora_core::extension::builtin::dj::SHOW_VIDEO,
            kinora_core::extension::SettingValue::Bool(show),
        );
        // What was got ready is of the other kind now.
        self.forget_ahead();
        if let Some(page) = self.0.page.borrow().as_ref() {
            page.set_audio_only(!show);
        }
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
            PanelAction::Leave => return self.leave(),
            PanelAction::ShowVideo(show) => return self.set_show_video(show),
            PanelAction::Hover(id) => {
                self.0.hovered.replace(Some(id));
                return;
            }
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

    /// Test hook: switch "Show video".
    #[cfg(feature = "dev-hooks")]
    pub fn dev_toggle_video(&self) {
        if let Some(client) = self.client() {
            self.set_show_video(!client.core().dj_settings().show_video);
            self.refresh_panel();
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
            "hover" => PanelAction::Hover(id),
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

/// Let a page go whose player may still run: pause only, and drop it a
/// moment later. Stopping a Clapper player, or ending one while another
/// starts, has crashed (see `VideoPage::stop`).
fn release(page: VideoPage) {
    page.stop();
    glib::timeout_add_local_once(Duration::from_millis(1500), move || drop(page));
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
    ShowVideo(bool),
    /// The pointer rests on a queue row.
    Hover(String),
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
    pending: adw::Spinner,
    show_video: gtk::ToggleButton,
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
        // Audio first; the video is one switch away.
        let show_video = gtk::ToggleButton::builder()
            .icon_name("camera-video-symbolic")
            .tooltip_text(gettextrs::gettext(
                "Show Video (off: audio only, tracks start faster)",
            ))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "k-dj-video"])
            .build();
        show_video.connect_toggled(glib::clone!(
            #[strong]
            handler,
            move |button| emit(&handler, PanelAction::ShowVideo(button.is_active()))
        ));
        header.append(&on);
        header.append(&note);
        header.append(&show_video);
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
        // Shown while the next track is asked for and has not started.
        let pending = adw::Spinner::builder()
            .width_request(16)
            .height_request(16)
            .valign(gtk::Align::Center)
            .visible(false)
            .tooltip_text(gettextrs::gettext("The next track is starting"))
            .build();
        // The window's own Previous and Next: the same as on the player.
        let previous = icon_button(
            "media-skip-backward-symbolic",
            &gettextrs::gettext("Previous (Shift+P)"),
        );
        previous.set_action_name(Some("win.player-previous"));
        let skip = icon_button(
            "media-skip-forward-symbolic",
            &gettextrs::gettext("Next: mix into the next track (Shift+N)"),
        );
        skip.set_action_name(Some("win.player-next"));
        now.append(&art_frame);
        now.append(&text);
        now.append(&pending);
        let transport = gtk::Box::builder().valign(gtk::Align::Center).build();
        transport.append(&previous);
        transport.append(&skip);
        now.append(&transport);

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
            pending,
            show_video,
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

    fn set_pending(&self, pending: bool) {
        self.pending.set_visible(pending);
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
        let show_video = client.core().dj_settings().show_video;
        if self.show_video.is_active() != show_video {
            self.show_video.set_active(show_video);
        }
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

        // The pointer resting on a row gets its track ready, so that picking
        // it mixes in at once.
        let motion = gtk::EventControllerMotion::new();
        let resting: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        motion.connect_enter(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            #[strong]
            resting,
            move |_, _, _| {
                let (handler, id, slot) = (handler.clone(), id.clone(), resting.clone());
                let source = glib::timeout_add_local_once(HOVER_DELAY, move || {
                    slot.take();
                    emit(&handler, PanelAction::Hover(id));
                });
                if let Some(old) = resting.replace(Some(source)) {
                    old.remove();
                }
            }
        ));
        motion.connect_leave(move |_| {
            if let Some(source) = resting.take() {
                source.remove();
            }
        });
        row.add_controller(motion);

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
