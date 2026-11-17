use std::{borrow::Borrow, path::Path};

use adw::Toast;
use clapper::MediaItem;
use clapper_gtk::prelude::AvExt;
use gdk::prelude::SettingsExt;
use glib::{
    Object, clone,
    object::{Cast, CastNone},
    subclass::types::ObjectSubclassIsExt,
};
use gtk::prelude::{BoxExt, SettingsExtManual, WidgetExt};
use url::Url;

use crate::{
    backend::{Client, Video},
    gspawn,
    gui::utility::Utility,
};

use super::window::Window;

/// Kinora: a media item that knows its song. Prepared streams are local
/// playlists (`ID.m3u8`, `ID.audio.m3u8`) with no tags, so Clapper's own title
/// (fullscreen video) would otherwise show the file name.
fn titled_item(uri: &str, video: &Video) -> MediaItem {
    use glib::translate::ToGlibPtr;
    let item = MediaItem::new(uri);
    let mut tags = gst::TagList::new();
    {
        let tags = tags.get_mut().expect("a new tag list is writable");
        tags.set_scope(gst::TagScope::Global);
        tags.add::<gst::tags::Title>(&video.title().as_str(), gst::TagMergeMode::Replace);
        let artist = video.uploader().name();
        if !artist.is_empty() {
            tags.add::<gst::tags::Artist>(&artist.as_str(), gst::TagMergeMode::Replace);
        }
    }
    // SAFETY: both pointers are valid for the call; Clapper copies the tags.
    unsafe {
        clapper::ffi::clapper_media_item_populate_tags(item.to_glib_none().0, tags.as_ptr());
    }
    item
}

gtk::glib::wrapper! {
    pub struct VideoPage(ObjectSubclass<imp::VideoPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl VideoPage {
    pub fn new(video: &Video, client: &Client) -> Self {
        Object::builder()
            .property("client", client)
            .property("video", video)
            .build()
    }

    /// Kinora: the page of a DJ track (see `gui/dj.rs`): no Up next and no
    /// autoplay of its own, the DJ's queue is shown instead.
    pub fn new_dj(video: &Video, client: &Client, audio_only: bool) -> Self {
        let page: Self = Object::builder()
            .property("client", client)
            .property("dj", true)
            .property("audio-only", audio_only)
            .property("video", video)
            .build();
        // With "Match Beats" the track's tempo will be changed: its player
        // has to know before it starts (see `Tap::set_engaged`).
        if let Some(tap) = page.imp().tap() {
            tap.set_engaged(client.core().dj_settings().match_beats);
        }
        page
    }

    pub fn play(&self) {
        self.set_external(false);
        if let Some(video) = self.video() {
            let imp = self.imp();
            // Play with internal clapper
            let player = imp
                .clapper
                .player()
                .expect("Clapper does not have a player");
            let queue = player.queue().expect("Clapper does not have a queue");
            queue.clear();
            self.imp().play_started.set(Some(std::time::Instant::now()));
            self.set_loading(true);
            // Kinora: audio only decodes no video, whatever the stream holds.
            player.set_video_enabled(!self.audio_only());
            let fallback = self.convert_to_clapper_playable_url(&video);
            let audio = Some(self.audio_only())
                .filter(|audio_only| *audio_only)
                .and_then(|_| self.client().prepared_audio_playback_uri(&video));
            imp.audio_stream.set(audio.is_some());
            imp.playing_uri.replace(None);
            if self.try_get_downloaded(&video).is_some() {
                imp.fast_start_active.set(false);
                queue.add_item(&titled_item(&fallback, &video));
                self.start_player(&player);
            } else if let Some(prepared) =
                audio.or_else(|| self.client().prepared_playback_uri(&video))
            {
                // Kinora: prepared in the background; starts in a second or two.
                log::info!("Playing prepared {}", video.id().id);
                imp.fast_start_active.set(true);
                imp.playing_uri.replace(Some(prepared.clone()));
                queue.add_item(&titled_item(&prepared, &video));
                self.start_player(&player);
                self.fall_back_if_stuck(&video);
            } else {
                // Kinora: fast start (streams in ~0.2 s instead of ~9 s); the
                // yt-dlp path is the fallback, here and on a playback error.
                let page = self.clone();
                let client = self.client();
                let video = video.clone();
                gspawn!(async move {
                    let fast = client.fast_playback_uri(&video).await;
                    if page.video().as_ref() != Some(&video) || page.external() {
                        return;
                    }
                    page.imp().fast_start_active.set(fast.is_some());
                    let watchdog = fast.is_some();
                    queue.add_item(&titled_item(&fast.unwrap_or(fallback), &video));
                    page.start_player(&player);
                    if watchdog {
                        page.fall_back_if_stuck(&video);
                    }
                });
            }
            // Kinora: count it on the YouTube account once it was really watched.
            if !imp.warming.get() {
                self.mark_watched_on_account_later(&video);
            }
        } else {
            self.pause();
        }
    }

    /// Start the player: playing, or, for a page that is being warmed
    /// ([`Self::warm`]), paused with its first seconds buffered.
    fn start_player(&self, player: &clapper::Player) {
        if self.imp().warming.get() {
            player.pause();
        } else {
            player.play();
        }
    }

    /// Kinora: get the page ready ahead of time (the DJ's next track): load
    /// the stream and hold it paused and silent at its start, so
    /// [`Self::start_warm`] plays at once.
    pub fn warm(&self) {
        self.imp().warming.set(true);
        self.set_fade_gain(Some(0.0));
        self.play();
    }

    /// A warmed page has its stream loaded and waits at the start.
    pub fn is_warm(&self) -> bool {
        self.imp().warming.get()
            && self.player_state() == clapper::PlayerState::Paused
            && self.duration_secs().is_some()
    }

    /// Kinora: keep a warm page waiting at its start although it is shown
    /// (`start_warm` then does nothing): the DJ lets it go on the beat.
    pub fn hold_warm(&self, hold: bool) {
        self.imp().warm_held.set(hold);
    }

    /// A warm page that waits to be let go (see `hold_warm`).
    pub fn is_warm_held(&self) -> bool {
        self.imp().warm_held.get() && self.imp().warming.get()
    }

    /// Play a page that was warmed (or is still warming).
    pub fn start_warm(&self) {
        let imp = self.imp();
        if imp.warm_held.get() || !imp.warming.replace(false) {
            return;
        }
        imp.play_started.set(Some(std::time::Instant::now()));
        if let Some(player) = imp.clapper.player() {
            player.play();
        }
        if let Some(video) = self.video() {
            self.mark_watched_on_account_later(&video);
        }
    }

    /// Kinora: have the track start at `position` instead of at its start
    /// (the DJ's "Start on the Beat"): an accurate seek, which a loaded
    /// player takes paused (a warm page) or playing. Once per page.
    pub fn seek_entry(&self, position: f64) {
        let imp = self.imp();
        let Some(player) = imp.clapper.player() else {
            return;
        };
        if imp.entry.replace(Some(position)).is_none() {
            let done = imp.entry_done.clone();
            player.connect_seek_done(move |_| done.set(true));
        }
        imp.entry_done.set(false);
        player.seek_custom(position, clapper::PlayerSeekMethod::Accurate);
    }

    /// Kinora: where the track was told to start, if not at its start.
    pub fn entry(&self) -> Option<f64> {
        self.imp().entry.get()
    }

    /// Kinora: the player is where `seek_entry` sent it.
    pub fn entry_ready(&self) -> bool {
        self.imp().entry_done.get()
    }

    /// Kinora: the page's player (for the app's MPRIS player).
    pub fn clapper_player(&self) -> Option<clapper::Player> {
        self.imp().clapper.player()
    }

    pub fn play_pause(&self) {
        // (Not while the record is in a hand: the scratch plays then.)
        if self.imp().scratch.borrow().is_some() {
            return;
        }
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        match player.state() {
            clapper::PlayerState::Paused => player.play(),
            clapper::PlayerState::Playing => player.pause(),
            _ => {}
        }
    }

    fn seek_by(&self, dt: f64) {
        let player = self.imp().clapper.player();
        if let Some(player) = player
            && let Some(duration) = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
        {
            let mut pending_position = player.position() + dt;
            if pending_position < 0.0 {
                pending_position = 0.0;
            } else if pending_position > duration {
                pending_position = duration;
            }
            player.seek(pending_position);
        }
    }

    fn seek_to_percent(&self, percent: f64) {
        let player = self.imp().clapper.player();
        if let Some(player) = player
            && let Some(duration) = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
        {
            let pending_position = percent * duration;
            player.seek(pending_position);
        }
    }

    pub fn play_external(&self) {
        self.pause();
        self.set_external(true);

        if let Some(video) = self.video() {
            let imp = self.imp();
            // Kinora: count it on the YouTube account.
            self.client().mark_watched_on_account(&video);

            let player = imp.settings.string("player");

            if player.is_empty() {
                self.window().show_toast(
                    Toast::builder()
                        .title(gettextrs::gettext(
                            "Configure your external player in the preferences first",
                        ))
                        .build(),
                );
                return;
            }

            let player_is_clapper = player
                .split_whitespace()
                .any(|w| w == "clapper" || w == "com.github.rafostar.Clapper");
            let url = if player_is_clapper {
                self.convert_to_clapper_playable_url(&video)
            } else {
                self.downloaded_path_or_url(&video)
            };
            let remove_gsk_renderer_env_variable = imp.settings.boolean("cairo-software-renderer");

            gspawn!(clone!(
                #[weak(rename_to = s)]
                self,
                async move {
                    let res =
                        Utility::run_command_on_url(player, url, remove_gsk_renderer_env_variable)
                            .await;
                    if let Err(e) = res {
                        s.window()
                            .display_error(gettextrs::gettext("Failed to spawn video player"), e);
                    }
                }
            ));
        }
    }

    pub fn pause(&self) {
        // Stop playing.
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        player.pause();
    }

    /// Kinora: a scratch that is under way ends here (the DJ changes track).
    pub fn cancel_scratch(&self) {
        self.imp().end_scratch();
    }

    /// Test hook: scratch the record as a hand would. `turns` is a list of
    /// "milliseconds:degrees" (see `Turntable::dev_scratch`); empty: a few
    /// baby scratches, a pull back and a push.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_scratch(&self, turns: &str) {
        let mut turns: Vec<(u64, f64)> = turns
            .split(',')
            .filter_map(|turn| {
                let (at, degrees) = turn.split_once(':')?;
                Some((at.trim().parse().ok()?, degrees.trim().parse().ok()?))
            })
            .collect();
        if turns.is_empty() {
            turns = vec![
                (250, 0.0),
                (400, -45.0),
                (520, 0.0),
                (640, -45.0),
                (760, 0.0),
                (880, -45.0),
                (1000, 0.0),
                (1500, -150.0),
                (1800, -150.0),
                (2300, 60.0),
            ];
        }
        log::info!("DEV: scratch {turns:?}");
        self.imp().dj_deck.dev_scratch(turns);
        // What the needle does, ten times a second.
        let page = glib::prelude::ObjectExt::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(page) = page.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match page.imp().scratch_report() {
                Some(report) => {
                    log::info!("DEV: scratch: {report}");
                    glib::ControlFlow::Continue
                }
                None => {
                    log::info!(
                        "DEV: scratch over: the player is {:?} at {:.2} s",
                        page.player_state(),
                        page.heard_position()
                    );
                    glib::ControlFlow::Break
                }
            }
        });
    }

    /// Kinora: play on (a paused player).
    pub fn resume(&self) {
        if let Some(player) = self.imp().clapper.player() {
            player.play();
        }
    }

    /// Kinora: the prepared stream that plays, if it is one (a manifest any
    /// GStreamer pipeline can read).
    pub fn playing_uri(&self) -> Option<String> {
        self.imp().playing_uri.borrow().clone()
    }

    /// Kinora: the last seconds the player decoded (a DJ page keeps them).
    pub fn deck_audio(&self) -> Option<crate::gui::deck_audio::Pcm> {
        self.imp().tap()?.snapshot()
    }

    /// Kinora: the position that is heard right now, to the millisecond
    /// (Clapper's own is refreshed a few times a second).
    pub fn heard_position(&self) -> f64 {
        self.imp()
            .tap()
            .and_then(|tap| tap.position())
            .unwrap_or_else(|| self.position_secs())
    }

    /// Kinora: how far into the track the player has decoded: a tempo set
    /// now is heard from there on.
    pub fn flow_position(&self) -> Option<f64> {
        self.imp().tap()?.flow()
    }

    /// Kinora: the tempo can be changed (see `set_tempo`).
    pub fn can_stretch(&self) -> bool {
        self.imp().tap().is_some_and(|tap| tap.stretches())
    }

    /// Kinora: the rate the track plays at, 1 being its own tempo.
    pub fn tempo(&self) -> f64 {
        self.imp().tap().map_or(1.0, |tap| tap.rate())
    }

    /// Kinora: play at `rate` times the track's tempo, at the same pitch and
    /// without a break (the DJ's beat matching).
    pub fn set_tempo(&self, rate: f64) {
        self.hold_tempo();
        if let Some(tap) = self.imp().tap() {
            tap.set_rate(rate);
        }
    }

    /// Kinora: stay at the tempo of now (see `ease_tempo_home`).
    pub fn hold_tempo(&self) {
        if let Some(source) = self.imp().tempo_ease.take() {
            source.remove();
        }
    }

    /// Kinora: back to the track's own tempo, little by little over `over`.
    pub fn ease_tempo_home(&self, over: std::time::Duration) {
        const STEP: std::time::Duration = std::time::Duration::from_millis(250);
        self.hold_tempo();
        let from = self.tempo();
        if (from - 1.0).abs() < 1e-6 {
            return;
        }
        let began = std::time::Instant::now();
        let page = glib::prelude::ObjectExt::downgrade(self);
        let source = glib::timeout_add_local(STEP, move || {
            let Some(page) = page.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let imp = page.imp();
            let done = (began.elapsed().as_secs_f64() / over.as_secs_f64().max(0.001)).min(1.0);
            // (Smooth at both ends.)
            let eased = done * done * (3.0 - 2.0 * done);
            if let Some(tap) = imp.tap() {
                tap.set_rate(if done >= 1.0 {
                    1.0
                } else {
                    from + (1.0 - from) * eased
                });
            }
            if done >= 1.0 {
                imp.tempo_ease.take();
                log::info!("DJ: {} is back at its own tempo", page.video().map(|v| v.id().id).unwrap_or_default());
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        self.imp().tempo_ease.replace(Some(source));
    }

    /// Kinora: keep a page that is let go alive a little longer: at least 3 s
    /// after now and 10 s after it was made. A Clapper player ended while its
    /// threads are still starting crashed the app (SIGSEGV in
    /// `g_main_context_unref` on Clapper's reactables thread, once right after
    /// the DJ built its first pages); players never end that young this way.
    pub fn retire(&self) {
        use std::time::Duration;
        let wait = Duration::from_secs(3)
            .max(Duration::from_secs(10).saturating_sub(self.imp().born.elapsed()));
        let page = self.clone();
        glib::timeout_add_local_once(wait, move || {
            drop(page);
            // (Its player's memory is freed on the player's threads.)
            crate::memory::trim_soon();
        });
    }

    /// Kinora: stop for good (another video replaces this page): pause and
    /// unload the video, so it no longer plays or streams.
    pub fn stop(&self) {
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        self.cancel_scratch();
        // Only pause: the page is destroyed right after, which ends the player.
        // Stopping or clearing the queue here raced with Clapper's player thread
        // (a crash, or the next page's player never starting).
        player.pause();
    }

    /// Kinora: if a prepared or fast-start stream isn't playing after 10 seconds
    /// (it can stall without an error), start over with yt-dlp.
    fn fall_back_if_stuck(&self, video: &Video) {
        let page = glib::prelude::ObjectExt::downgrade(self);
        let video = video.clone();
        glib::timeout_add_seconds_local_once(10, move || {
            let Some(page) = page.upgrade() else { return };
            let imp = page.imp();
            if page.video().as_ref() != Some(&video) || !imp.fast_start_active.get() {
                return;
            }
            let Some(player) = imp.clapper.player() else {
                return;
            };
            // (A page that is being warmed waits paused: that is not stuck.)
            if matches!(player.state(), clapper::PlayerState::Playing)
                || player.position() > 0.0
                || (imp.warming.get() && page.duration_secs().is_some())
            {
                return;
            }
            log::warn!(
                "Prepared stream for {} stalled; using yt-dlp",
                video.id().id
            );
            imp.fast_start_active.set(false);
            if let Some(queue) = player.queue() {
                queue.clear();
                queue.add_item(&titled_item(
                    &page.convert_to_clapper_playable_url(&video),
                    &video,
                ));
            }
            player.play();
        });
    }

    /// Kinora: mark the video watched on the YouTube account after 30 seconds
    /// of it still being on this page. Doing it at once also competed with the
    /// player for the network and CPU right when the video was starting.
    fn mark_watched_on_account_later(&self, video: &Video) {
        let page = glib::prelude::ObjectExt::downgrade(self);
        let video = video.clone();
        glib::timeout_add_seconds_local_once(30, move || {
            if let Some(page) = page.upgrade() {
                if page.video().as_ref() == Some(&video) {
                    page.client().mark_watched_on_account(&video);
                }
            }
        });
    }

    // --- Kinora: what the player sheet needs (see `player_sheet.rs`) ---------

    /// The player's place in the page: x, y, width, height.
    ///
    /// On the DJ's record player it is a 16:9 band across the middle of the
    /// record: the label with vinyl either side, which is what the
    /// mini-player then shows.
    pub fn player_rect(&self) -> Option<[f64; 4]> {
        let imp = self.imp();
        if imp.vinyl() {
            let (x, y, r) = imp.dj_deck.record();
            let centre = imp
                .dj_deck
                .compute_point(self, &gtk::graphene::Point::new(x as f32, y as f32))?;
            let width = r * 1.8;
            let height = width * 9.0 / 16.0;
            return Some([
                centre.x() as f64 - width / 2.0,
                centre.y() as f64 - height / 2.0,
                width,
                height,
            ]);
        }
        let r = imp.player_frame.compute_bounds(self)?;
        Some([
            r.x() as f64,
            r.y() as f64,
            r.width() as f64,
            r.height() as f64,
        ])
    }

    /// The page is about to be laid out at this size (by the sheet). The
    /// DJ's record player is a square in its column, at most 44% of the
    /// page's height, so the buttons and the first of the queue stay in
    /// view. It is sized here, not a frame later, so a new track's page is
    /// right from its first frame.
    pub fn fit(&self, width: i32, height: i32) {
        let imp = self.imp();
        if !self.dj() {
            return;
        }
        let side = ((width - 32).min(528) as f64)
            .min(height as f64 * 0.44)
            .max(180.0)
            .round() as i32;
        if imp.dj_deck.width_request() != side {
            imp.dj_deck.set_size_request(side, side);
        }
    }

    /// Whether a point of the page is on the stage (the player and its glow).
    pub fn stage_contains(&self, x: f64, y: f64) -> bool {
        let imp = self.imp();
        let stage: &gtk::Widget = if imp.vinyl() {
            imp.dj_deck.upcast_ref()
        } else {
            imp.stage.upcast_ref()
        };
        stage
            .compute_bounds(self)
            .is_some_and(|r| r.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)))
    }

    /// Fade everything but the player (the header, the glow, the details).
    pub fn set_chrome_opacity(&self, opacity: f64) {
        let imp = self.imp();
        imp.header_bar.set_opacity(opacity);
        imp.stage_glow.set_opacity(opacity);
        imp.stage_scrim.set_opacity(opacity);
        imp.details_box.set_opacity(opacity);
        // (The DJ's page: all but the record.)
        imp.dj_glow.set_opacity(opacity);
        imp.dj_scrim.set_opacity(opacity);
        imp.dj_bar.set_opacity(opacity);
        imp.dj_body.set_opacity(opacity);
    }

    /// How far the page is scrolled down, in pixels.
    pub fn scroll_position(&self) -> f64 {
        gtk::prelude::AdjustmentExt::value(&self.imp().page_scroller.vadjustment())
    }

    /// Back to the top at once (the player has to be in view to shrink).
    pub fn scroll_top_now(&self) {
        gtk::prelude::AdjustmentExt::set_value(&self.imp().page_scroller.vadjustment(), 0.0);
    }

    /// The played part of the video, 0 to 1.
    pub fn progress_fraction(&self) -> f64 {
        let Some(player) = self.imp().clapper.player() else {
            return 0.0;
        };
        let duration = player
            .queue()
            .and_then(|q| q.current_item())
            .map(|i| i.duration())
            .unwrap_or(0.0);
        if duration > 0.0 {
            (player.position() / duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Start what plays over from the beginning.
    pub fn restart(&self) {
        if let Some(player) = self.imp().clapper.player() {
            player.seek(0.0);
        }
    }

    /// The player's position in seconds.
    pub fn position_secs(&self) -> f64 {
        self.imp().clapper.player().map_or(0.0, |p| p.position())
    }

    /// The length of what plays, in seconds, once the player knows it.
    pub fn duration_secs(&self) -> Option<f64> {
        self.imp()
            .clapper
            .player()
            .and_then(|p| p.queue())
            .and_then(|q| q.current_item())
            .map(|i| i.duration())
            .filter(|d| *d > 0.0)
    }

    pub fn player_state(&self) -> clapper::PlayerState {
        self.imp()
            .clapper
            .player()
            .map_or(clapper::PlayerState::Stopped, |p| p.state())
    }

    /// Test hook: scroll the page down to `y`.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_scroll_to(&self, y: f64) {
        let adjustment = self.imp().page_scroller.vadjustment();
        gtk::prelude::AdjustmentExt::set_value(&adjustment, y);
        log::info!(
            "DEV: page scrolled to {} of {}",
            gtk::prelude::AdjustmentExt::value(&adjustment),
            gtk::prelude::AdjustmentExt::upper(&adjustment)
        );
    }

    /// Test hook: toggle fullscreen (`window_mode`: fill the window only).
    #[cfg(feature = "dev-hooks")]
    pub fn dev_fullscreen(&self, window_mode: bool) {
        self.imp().toggle_fullscreen_mode(window_mode);
        log::info!("DEV: fullscreened {}", self.fullscreened());
    }

    /// Test hook: for every visible button on the page, log which widget a
    /// click on its centre would reach (what `pick` finds there).
    #[cfg(feature = "dev-hooks")]
    pub fn dev_pick_buttons(&self) {
        use gtk::prelude::*;
        let Some(root) = self.root() else { return };
        let root: gtk::Widget = root.upcast();
        let mut stack: Vec<gtk::Widget> = vec![self.clone().upcast()];
        let describe = |w: &gtk::Widget| {
            format!("{}{:?}", w.type_().name(), w.css_classes().iter().map(|c| c.to_string()).collect::<Vec<_>>())
        };
        while let Some(w) = stack.pop() {
            let mut c = w.first_child();
            while let Some(child) = c {
                c = child.next_sibling();
                stack.push(child);
            }
            if !w.is::<gtk::Button>() || !w.is_drawable() {
                continue;
            }
            let (ww, wh) = (w.width() as f32, w.height() as f32);
            let Some(p) = w.compute_point(&root, &gtk::graphene::Point::new(ww / 2.0, wh / 2.0)) else { continue };
            if p.x() < 0.0 || p.y() < 0.0 || p.x() > root.width() as f32 || p.y() > root.height() as f32 {
                continue;
            }
            let picked = root.pick(p.x() as f64, p.y() as f64, gtk::PickFlags::DEFAULT);
            let ok = picked.as_ref().is_some_and(|x| x == &w || x.is_ancestor(&w));
            let name = w.downcast_ref::<gtk::Button>().and_then(|b| b.action_name()).map(|a| a.to_string()).or_else(|| w.tooltip_text().map(|t| t.to_string())).unwrap_or_default();
            log::info!(
                "DEV: pick {} {:?} at ({:.0},{:.0}): {}",
                if ok { "OK " } else { "BLOCKED" },
                name, p.x(), p.y(),
                picked.map(|x| { let mut d = describe(&x); let mut a = x.parent(); for _ in 0..3 { if let Some(pa) = a { d.push_str(" < "); d.push_str(&describe(&pa)); a = pa.parent(); } } d }).unwrap_or_default()
            );
        }
    }

    /// Test hook: jump to `seconds` before the end.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_seek_near_end(&self, seconds: f64) {
        if let (Some(player), Some(duration)) = (self.imp().clapper.player(), self.duration_secs())
        {
            player.seek((duration - seconds).max(0.0));
        }
    }

    /// The DJ's crossfade: play at `gain` (linear amplitude, 0 to 1) of the
    /// user's volume. While a gain is set, the player's volume is the DJ's:
    /// it is taken off the `player-volume` setting, which two players fading
    /// against each other would otherwise fight over. `None` hands it back.
    pub fn set_fade_gain(&self, gain: Option<f64>) {
        let imp = self.imp();
        let Some(player) = imp.clapper.player() else {
            return;
        };
        imp.quiet_volume_until.set(Some(
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        ));
        match gain {
            Some(gain) => {
                if imp.fade_base.get().is_none() {
                    gtk::gio::Settings::unbind(&player, "volume");
                    imp.fade_base
                        .set(Some(imp.settings.double("player-volume")));
                }
                let base = imp.fade_base.get().unwrap_or(1.0);
                // Clapper's volume is on a cubic scale; the gain is linear.
                player.set_volume(base * gain.clamp(0.0, 1.0).cbrt());
            }
            None => {
                if imp.fade_base.take().is_some() {
                    player.set_volume(imp.settings.double("player-volume"));
                    imp.settings
                        .bind("player-volume", &player, "volume")
                        .build();
                }
            }
        }
    }

    /// Put the DJ's widgets into its page (`None` takes them out again): its
    /// bar ("DJ on", Leave), what plays (title, artists), and the queue.
    pub fn set_dj_panel(&self, parts: Option<[&gtk::Widget; 3]>) {
        let imp = self.imp();
        let slots = [&imp.dj_bar_slot, &imp.dj_now_slot, &imp.dj_queue_slot];
        for (index, slot) in slots.into_iter().enumerate() {
            while let Some(child) = slot.first_child() {
                slot.remove(&child);
            }
            if let Some(part) = parts.map(|p| p[index]) {
                if let Some(parent) = part.parent().and_downcast::<gtk::Box>() {
                    parent.remove(part);
                }
                slot.append(part);
            }
        }
    }

    /// The first "Up next" video, if they have arrived.
    pub fn next_video(&self) -> Option<Video> {
        gtk::gio::prelude::ListModelExt::item(&self.imp().up_next_store, 0)
            .and_then(|o| o.downcast::<Video>().ok())
    }

    pub fn finished(&self) -> bool {
        if self.external() {
            return true;
        }

        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");

        if let Some(duration) = player
            .queue()
            .and_then(|q| q.current_item())
            .map(|i| i.duration())
        {
            // Kinora: the player can stop a hair past the duration.
            player.position() >= duration - 0.75
        } else {
            true
        }
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }

    fn try_get_downloaded(&self, video: &Video) -> Option<String> {
        if let Some(path) = video.downloaded_path() {
            if Path::new(&path).exists() {
                Some(path.clone())
            } else {
                self.client().remove_downloaded(video);
                video.set_downloaded_path(None::<String>);
                self.window().show_toast(
                    Toast::builder()
                        .title(gettextrs::gettext(
                            "Downloaded video not available locally anymore",
                        ))
                        .build(),
                );
                None
            }
        } else {
            None
        }
    }

    fn downloaded_path_or_url(&self, video: &Video) -> String {
        if let Some(downloaded) = self.try_get_downloaded(video) {
            downloaded
        } else {
            video.playable_url().to_string()
        }
    }

    fn convert_to_clapper_playable_url(&self, video: &Video) -> String {
        if let Some(downloaded) = self
            .try_get_downloaded(video)
            .and_then(|d| Url::from_file_path(d).ok())
        {
            return downloaded.to_string();
        }

        let url = video.playable_url().to_string();
        match video.id().platform.borrow() {
            "peertube" => {
                // This would be better expressed as `Url::set_scheme`, but this would error as `http(s)` is "special" and `peertube` is not.
                if let Some((_, rest)) = url.split_once("://") {
                    format!("peertube://{rest}")
                } else {
                    url
                }
            }
            _ => url,
        }
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::marker::PhantomData;

    use adw::subclass::bin::BinImpl;
    use clapper_gtk::prelude::AvExt;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Propagation;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CallbackAction;
    use gtk::CompositeTemplate;
    use gtk::Shortcut;
    use gtk::ShortcutController;
    use gtk::ShortcutTrigger;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::backend::FiniteVideoStore;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::backend::VideoSort;
    use crate::config::BASE_ID;
    use crate::gspawn;
    use crate::gui::scratch::Scratch;
    use crate::gui::turntable::{Hand, SPIN, Turntable};

    /// Let go, the needle and the player meet at least this far on, in
    /// seconds (the player has to seek there first).
    const HAND_BACK_FROM: f64 = 0.8;
    /// The player is told to play this long before the needle is where it
    /// waits, in seconds: about what it takes to be heard.
    const HAND_BACK_LEAD: f64 = 0.01;
    /// Once the player is heard again, the needle is kept this long (its
    /// sound fades at once), in milliseconds.
    const HAND_BACK_SETTLE: u128 = 300;
    /// A player that was told to play and is not heard after this long is
    /// late, in milliseconds.
    const HAND_BACK_PATIENCE: u128 = 250;

    /// The DJ's record in a hand (see `deck_hand`).
    pub(super) struct ScratchSession {
        /// What plays the hand's moves; `None`: nothing of the track was at
        /// hand, and the record is moved without sound.
        engine: Option<Scratch>,
        /// Where the track would be with the record turned back to where it
        /// was taken, in seconds.
        origin: f64,
        duration: f64,
        was_playing: bool,
        /// The record is being dragged (not only pressed).
        pub(super) taken: bool,
        /// Where the hand has the record, in seconds of the track.
        hand: f64,
        /// Let go: on its way back to the player.
        pub(super) released: Option<Released>,
        timer: Option<glib::SourceId>,
    }

    pub(super) struct Released {
        at: std::time::Instant,
        /// The needle's place and the label's turn when it was let go.
        position: f64,
        turn: f64,
        /// Where the player was told to wait for the needle.
        target: Option<f64>,
        seek_done: std::rc::Rc<Cell<bool>>,
        seeks: u32,
        /// The player was told to play, and is heard.
        handed: Option<std::time::Instant>,
        joined: Option<std::time::Instant>,
        signal: Option<glib::SignalHandlerId>,
    }

    impl Released {
        fn forget(mut self, player: Option<&clapper::Player>) {
            if let (Some(player), Some(signal)) = (player, self.signal.take()) {
                player.disconnect(signal);
            }
        }
    }

    /// Decode more of the track around what the scratch has, in the
    /// background: 25 s after it first (handed over as it comes), then 20 s
    /// before it.
    fn feed_scratch(engine: &Scratch, uri: String) {
        let (feeder, rate, (start, end)) = (engine.feeder(), engine.sample_rate(), engine.range());
        let _ = std::thread::Builder::new()
            .name("kinora-scratch".into())
            .spawn(move || {
                use crate::gui::deck_audio::decode;
                let began = std::time::Instant::now();
                let timeout = std::time::Duration::from_secs(20);
                let ahead = feeder.clone();
                let after = decode(
                    &uri,
                    (end - 0.05).max(0.0),
                    25.0,
                    rate,
                    2,
                    timeout,
                    Some(std::sync::Arc::new(move |pcm| ahead.extend(&pcm))),
                );
                let after_took = began.elapsed().as_secs_f64();
                let from = (start - 20.0).max(0.0);
                let before = (feeder.wanted() && start > 0.5)
                    .then(|| decode(&uri, from, start - from + 0.05, rate, 2, timeout, None))
                    .map(|pcm| pcm.map(|pcm| feeder.extend(&pcm)));
                log::info!(
                    "Scratch: more of the track: after {} ({after_took:.2} s), before {} ({:.2} s)",
                    after.map_or_else(|e| e, |_| "ok".into()),
                    match before {
                        Some(Ok(true)) => "ok".into(),
                        Some(Ok(false)) => "does not join on".into(),
                        Some(Err(e)) => e,
                        None => "not needed".into(),
                    },
                    began.elapsed().as_secs_f64() - after_took
                );
            });
    }
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;


    /// "3:32", for the DJ's line.
    fn clock(seconds: f64) -> String {
        crate::gui::dj::format_length(Some(std::time::Duration::from_secs_f64(seconds.max(0.0))))
    }

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/video_page.ui")]
    #[properties(wrapper_type = super::VideoPage)]
    pub struct VideoPage {
        /// Kinora: when the page (and its Clapper player) was made; see
        /// [`super::retire`].
        pub(super) born: std::time::Instant,
        /// Kinora: no volume bubble until then (the DJ set the volume, not the
        /// user; Clapper may report it a moment later).
        pub(super) quiet_volume_until: std::cell::Cell<Option<std::time::Instant>>,
        #[template_child]
        pub(super) clapper: TemplateChild<clapper_gtk::Video>,
        #[property(get, set = Self::set_video, nullable)]
        video: RefCell<Option<Video>>,
        #[property(get = Self::get_state, builder(clapper::PlayerState::Stopped))]
        state: PhantomData<clapper::PlayerState>,
        #[property(get = Self::get_duration)]
        duration: PhantomData<f64>,
        #[property(get = Self::get_position)]
        position: PhantomData<f64>,

        #[template_child]
        billboard: TemplateChild<clapper_gtk::Billboard>,
        #[template_child]
        pub(super) page_scroller: TemplateChild<gtk::ScrolledWindow>,
        // Kinora: the stage (player over its ambient glow) and the page body.
        #[template_child]
        pub(super) stage: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) player_frame: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub(super) stage_glow: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) stage_scrim: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) details_box: TemplateChild<gtk::Box>,
        #[template_child]
        stage_clamp: TemplateChild<adw::Clamp>,
        #[template_child]
        body_clamp: TemplateChild<adw::Clamp>,
        #[template_child]
        body: TemplateChild<gtk::Box>,
        #[template_child]
        up_next_clamp: TemplateChild<adw::Clamp>,

        // Kinora: "Up next"
        #[template_child]
        up_next_box: TemplateChild<gtk::Box>,
        #[template_child]
        up_next_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        autoplay_switch: TemplateChild<gtk::Switch>,
        pub(super) up_next_store: FiniteVideoStore,
        // Kinora: the DJ's page (see docs/design.md, "DJ").
        #[template_child]
        pub(super) dj_glow: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) dj_scrim: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) dj_bar: TemplateChild<adw::Clamp>,
        #[template_child]
        pub(super) dj_bar_slot: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) dj_deck: TemplateChild<Turntable>,
        #[template_child]
        pub(super) dj_body: TemplateChild<adw::Clamp>,
        #[template_child]
        pub(super) dj_now_slot: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) dj_seek: TemplateChild<gtk::Scale>,
        #[template_child]
        dj_elapsed: TemplateChild<gtk::Label>,
        #[template_child]
        dj_length: TemplateChild<gtk::Label>,
        #[template_child]
        dj_play: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) dj_queue_slot: TemplateChild<gtk::Box>,
        /// A seek asked for on the DJ's line that waits a moment (the knob is
        /// being dragged), and until when the line then ignores the player
        /// (which still reports the old place).
        dj_seek_pending: RefCell<Option<glib::SourceId>>,
        dj_seek_quiet_until: Cell<Option<std::time::Instant>>,
        /// What plays was loaded as sound alone: it has no picture to show.
        pub(super) audio_stream: Cell<bool>,
        /// Kinora: the DJ's audio-only way: the stage shows the track's artwork
        /// and no video is decoded.
        #[property(get, set = Self::set_audio_only)]
        audio_only: Cell<bool>,
        /// The page is loaded ahead of time, paused (see `warm`).
        pub(super) warming: Cell<bool>,
        /// A warm page that is to stay as it is until told (see `hold_warm`).
        pub(super) warm_held: Cell<bool>,
        /// Where the track was told to start (see `seek_entry`), and
        /// whether the player has got there.
        pub(super) entry: Cell<Option<f64>>,
        pub(super) entry_done: std::rc::Rc<Cell<bool>>,
        /// Kinora: "Up next" has arrived, so there is a next video.
        #[property(get)]
        has_next: Cell<bool>,
        /// Kinora: this page plays a DJ track (see `gui/dj.rs`).
        #[property(get, set = Self::set_dj)]
        dj: Cell<bool>,
        /// Kinora: the DJ can be started (its extension is on).
        #[property(get, set)]
        dj_available: Cell<bool>,
        /// The user's volume while the DJ fades this player (see `set_fade_gain`).
        pub(super) fade_base: Cell<Option<f64>>,
        /// Kinora: the player's audio filter: it keeps what just played and
        /// changes the tempo (see `gui/deck_audio.rs`). Empty when GStreamer
        /// lacks its elements.
        pub(super) tap: std::cell::OnceCell<Option<crate::gui::deck_audio::Tap>>,
        /// The tempo is on its way back to the track's own (see `ease_tempo_home`).
        pub(super) tempo_ease: RefCell<Option<glib::SourceId>>,
        /// What the player was given to play.
        pub(super) playing_uri: RefCell<Option<String>>,
        /// Kinora: the record is in a hand, or on its way back to the player
        /// (see `deck_hand`).
        pub(super) scratch: RefCell<Option<ScratchSession>>,
        /// The player's sound coming back in after a scratch.
        pub(super) scratch_ramp: RefCell<Option<glib::SourceId>>,
        /// Kinora: the page is shrunk into the mini-player (see `player_sheet.rs`).
        #[property(get, set)]
        minimized: Cell<bool>,
        /// Kinora: where and when the player was last double-clicked:
        /// -1 left third, 0 middle, 1 right third.
        double_click: Cell<Option<(i8, std::time::Instant)>>,
        autoplay_done: Cell<bool>,
        /// When play() was pressed, to log how long the video took to start.
        pub(super) play_started: Cell<Option<std::time::Instant>>,

        #[property(get, set)]
        fullscreened: Cell<bool>,
        /// Kinora: while fullscreened, fill the window instead of the screen.
        #[property(get, set)]
        window_mode: Cell<bool>,
        /// Kinora: the video is starting (thumbnail and spinner over the player).
        #[property(get, set)]
        loading: Cell<bool>,
        /// Kinora: the description is shown in full rather than a few lines.
        #[property(get, set)]
        description_expanded: Cell<bool>,
        /// Kinora: the current item came from fast start (fall back on errors).
        pub(super) fast_start_active: Cell<bool>,
        /// Kinora: scroll to the top the next time the page is shown.
        scroll_to_top_on_show: Cell<bool>,
        #[property(get, set)]
        show_sidebar_visible: Cell<bool>,
        #[property(get, set)]
        primary_menu_visible: Cell<bool>,

        #[property(get, set)]
        external: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,

        watch: RefCell<Option<gtk::ExpressionWatch>>,
        pub settings: gtk::gio::Settings,
    }

    impl Default for VideoPage {
        fn default() -> Self {
            Self {
                born: std::time::Instant::now(),
                quiet_volume_until: Default::default(),
                clapper: Default::default(),
                video: Default::default(),
                billboard: Default::default(),
                page_scroller: Default::default(),
                stage: Default::default(),
                player_frame: Default::default(),
                header_bar: Default::default(),
                stage_glow: Default::default(),
                stage_scrim: Default::default(),
                details_box: Default::default(),
                dj_glow: Default::default(),
                dj_scrim: Default::default(),
                dj_bar: Default::default(),
                dj_bar_slot: Default::default(),
                dj_deck: Default::default(),
                dj_body: Default::default(),
                dj_now_slot: Default::default(),
                dj_seek: Default::default(),
                dj_elapsed: Default::default(),
                dj_length: Default::default(),
                dj_play: Default::default(),
                dj_queue_slot: Default::default(),
                dj_seek_pending: Default::default(),
                dj_seek_quiet_until: Default::default(),
                audio_stream: Default::default(),
                audio_only: Default::default(),
                warming: Default::default(),
                warm_held: Default::default(),
                entry: Default::default(),
                entry_done: Default::default(),
                has_next: Default::default(),
                dj: Default::default(),
                dj_available: Default::default(),
                fade_base: Default::default(),
                tap: Default::default(),
                tempo_ease: Default::default(),
                playing_uri: Default::default(),
                scratch: Default::default(),
                scratch_ramp: Default::default(),
                minimized: Default::default(),
                double_click: Default::default(),
                stage_clamp: Default::default(),
                body_clamp: Default::default(),
                body: Default::default(),
                up_next_clamp: Default::default(),
                up_next_box: Default::default(),
                up_next_list: Default::default(),
                autoplay_switch: Default::default(),
                up_next_store: FiniteVideoStore::with_sort(VideoSort::AsAdded),
                autoplay_done: Default::default(),
                play_started: Default::default(),
                fullscreened: Default::default(),
                window_mode: Default::default(),
                loading: Default::default(),
                description_expanded: Default::default(),
                fast_start_active: Default::default(),
                scroll_to_top_on_show: Default::default(),
                state: Default::default(),
                position: Default::default(),
                duration: Default::default(),
                show_sidebar_visible: Default::default(),
                primary_menu_visible: Default::default(),
                external: Default::default(),
                client: Default::default(),
                watch: Default::default(),
                settings: gtk::gio::Settings::new(BASE_ID),
            }
        }
    }

    impl VideoPage {
        // Recommended syntax is ugly and the item will be moved to `pedantic` soon.
        #[allow(clippy::assigning_clones)]
        fn set_video(&self, video: Option<Video>) {
            *self.video.borrow_mut() = video.clone();

            if let Some(video) = video {
                let obj = self.obj();
                let client = obj.client();

                // Kinora: load "Up next" alongside the video information.
                self.autoplay_done.set(false);
                obj.set_description_expanded(false);
                // Kinora: a new video's page opens at the top (see `setup`).
                self.scroll_to_top_on_show.set(true);
                if self.page_scroller.is_mapped() {
                    self.scroll_to_top();
                }
                self.up_next_store.clear();
                self.up_next_box.set_visible(false);
                // The DJ has its own queue.
                if !self.dj.get() {
                    self.load_up_next(&video);
                }

                // Load additional data
                gspawn!(
                    clone!(
                        #[weak]
                        obj,
                        #[weak]
                        client,
                        #[weak]
                        video,
                        async move {
                            let result = client.populate_video_information(&video.id()).await;
                            if let Err(e) = result {
                                log::error!("Failed to populate video information: {e}");
                                obj.window().display_error(
                                    gettextrs::gettext("Failed to populate video information"),
                                    Box::new(e),
                                );
                            }

                            // The video thumbnail could have changed after updating the information; reload the thumbnail.
                            client.populate_thumbnail(&video).await;

                            // Kinora: an "Up next" video can still be waiting for its
                            // channel; there is nothing to look up yet.
                            let channel_id = video.uploader().id();
                            let result = if channel_id.id.is_empty() {
                                Ok(())
                            } else {
                                client.populate_channel_information(&channel_id).await
                            };
                            if let Err(e) = result {
                                log::error!("Failed to populate channel information: {e}");
                                obj.window().display_error(
                                    gettextrs::gettext("Failed to fetch channel information"),
                                    Box::new(e),
                                );
                            }
                        },
                    ),
                    glib::Priority::LOW
                );
            }
        }

        /// Kinora: fetch "Up next" for the page's video and get the first one ready.
        fn load_up_next(&self, video: &Video) {
            let obj = self.obj();
            let client = obj.client();
            gspawn!(clone!(
                #[weak]
                obj,
                #[weak]
                client,
                #[weak]
                video,
                async move {
                    let videos = client.up_next(&video).await;
                    let imp = obj.imp();
                    // The page may have moved on to another video meanwhile.
                    if obj.video().as_ref() != Some(&video) {
                        return;
                    }
                    imp.up_next_store.clear();
                    for v in videos {
                        imp.up_next_store.add(v);
                    }
                    imp.up_next_box
                        .set_visible(imp.up_next_store.n_items() > 0 && !obj.dj());
                    #[cfg(feature = "dev-hooks")]
                    imp.dev_hooks_after_up_next();
                    // Kinora: get the likely next video ready, so it starts at once.
                    if let Some(next) = imp.up_next_store.item(0).and_downcast::<Video>() {
                        client.prepare_playback(&next);
                    }
                }
            ));
        }

        /// Kinora: audio only, or with the video (the DJ's "Show video").
        /// A track that was loaded without its video keeps its artwork: the
        /// video comes with the next one.
        fn set_audio_only(&self, audio_only: bool) {
            // (Sound alone was loaded: there is no picture to switch on.)
            if !audio_only && self.audio_stream.get() {
                return;
            }
            if self.audio_only.replace(audio_only) == audio_only {
                return;
            }
            if let Some(player) = self.clapper.player() {
                player.set_video_enabled(!audio_only);
            }
        }

        /// Test hook: where the hand and the needle are.
        #[cfg(feature = "dev-hooks")]
        pub(super) fn scratch_report(&self) -> Option<String> {
            let slot = self.scratch.borrow();
            let session = slot.as_ref()?;
            Some(match &session.engine {
                Some(engine) => {
                    let (from, to) = engine.range();
                    format!(
                        "hand {:.3} s  needle {:.3} s  rate {:+.2}  audio {:.1}..{:.1} s{}{}",
                        session.hand,
                        engine.position(),
                        engine.rate(),
                        from,
                        to,
                        if session.taken { "  held" } else { "" },
                        if session.released.is_some() { "  let go" } else { "" },
                    )
                }
                None => format!("hand {:.3} s, no sound", session.hand),
            })
        }

        pub(super) fn tap(&self) -> Option<&crate::gui::deck_audio::Tap> {
            self.tap.get().and_then(|tap| tap.as_ref())
        }

        /// Kinora: the page shows the DJ's record player (an audio-only DJ
        /// track) where the video would be.
        pub(super) fn vinyl(&self) -> bool {
            self.dj.get() && self.audio_only.get()
        }

        /// Kinora: the DJ page's line and its record's arm follow the player.
        fn update_dj_progress(&self) {
            // (In a hand, the record says where the track is.)
            let scratched = self
                .scratch
                .borrow()
                .as_ref()
                .is_some_and(|s| s.taken || s.released.is_some());
            if !self.dj.get() || scratched {
                return;
            }
            let obj = self.obj();
            let fraction = obj.progress_fraction();
            self.dj_deck.set_progress(fraction);
            let quiet = self
                .dj_seek_quiet_until
                .get()
                .is_some_and(|until| std::time::Instant::now() < until);
            if quiet || self.dj_seek_pending.borrow().is_some() {
                return;
            }
            self.dj_seek.set_value(fraction);
            self.dj_elapsed.set_label(&clock(obj.position_secs()));
            self.dj_length
                .set_label(&obj.duration_secs().map(clock).unwrap_or_default());
        }

        // ---- Kinora: scratching (see docs/design.md, "DJ") ----
        //
        // While the record is held, the player rests and `gui/scratch.rs`
        // plays what the hand does, from the audio the player just decoded
        // and more that is decoded around it. Let go, the needle runs up to
        // speed there too, the player seeks to a place a little ahead, and
        // takes over when the needle gets there.

        /// A hand on the deck (see [`Hand`]).
        pub(super) fn deck_hand(&self, hand: Hand) -> bool {
            match hand {
                Hand::Tap | Hand::Away => {
                    // What was got ready for a scratch is not needed.
                    let ready = self
                        .scratch
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| !s.taken && s.released.is_none());
                    if ready {
                        self.end_scratch();
                    }
                    if hand == Hand::Tap {
                        self.obj().play_pause();
                    }
                }
                Hand::Down => return self.scratch_down(),
                Hand::Turn(degrees) => self.scratch_turn(degrees),
                Hand::Up => self.scratch_up(),
            }
            true
        }

        /// The user's volume as a gain (Clapper's volume is on a cubic scale).
        fn scratch_volume(&self) -> f64 {
            self.clapper
                .player()
                .filter(|player| !player.is_muted())
                .map_or(0.0, |player| player.volume().clamp(0.0, 1.5).powi(3))
        }

        /// The pointer went down on the record: get ready to scratch, so
        /// that the first move of the hand is heard at once.
        fn scratch_down(&self) -> bool {
            let obj = self.obj();
            if let Some(session) = self.scratch.borrow_mut().as_mut() {
                // Taken again while it runs up to speed.
                let again = session.released.is_some() && session.engine.is_some();
                session.taken &= !again;
                return again;
            }
            // (Not while the DJ fades this track, or holds it ready.)
            let state = obj.player_state();
            if !self.vinyl()
                || self.fade_base.get().is_some()
                || self.warming.get()
                || !matches!(
                    state,
                    clapper::PlayerState::Playing | clapper::PlayerState::Paused
                )
            {
                return false;
            }
            let Some(duration) = obj.duration_secs() else {
                return false;
            };
            let origin = obj.heard_position();
            let engine = self
                .tap()
                .and_then(|tap| tap.snapshot())
                .filter(|audio| audio.start <= origin + 0.02 && origin <= audio.end() + 0.02)
                .and_then(|audio| Scratch::new(audio, self.scratch_volume()));
            self.scratch.replace(Some(ScratchSession {
                engine,
                origin,
                duration,
                was_playing: state == clapper::PlayerState::Playing,
                taken: false,
                hand: origin,
                released: None,
                timer: None,
            }));
            true
        }

        /// The record is dragged, `degrees` from where it was taken.
        fn scratch_turn(&self, degrees: f64) {
            let obj = self.obj();
            let mut slot = self.scratch.borrow_mut();
            let Some(session) = slot.as_mut() else {
                return;
            };
            if !session.taken {
                session.taken = true;
                if let Some(released) = session.released.take() {
                    // Taken again: on from where the needle is.
                    released.forget(self.clapper.player().as_ref());
                    if let Some(engine) = &session.engine {
                        session.origin = engine.position();
                        engine.grab(session.origin, engine.rate());
                    }
                } else {
                    session.origin = obj.heard_position();
                    if session.was_playing {
                        obj.pause();
                    }
                    match &session.engine {
                        Some(engine) => {
                            engine.grab(session.origin, if session.was_playing { 1.0 } else { 0.0 });
                            let (from, to) = engine.range();
                            log::info!(
                                "Scratch: the record is taken at {:.2} s ({:.1} s before and {:.1} s after it at hand)",
                                session.origin,
                                session.origin - from,
                                to - session.origin
                            );
                            // (Played another way, the track's prepared
                            // audio does as well, if there is any.)
                            let uri = obj.playing_uri().or_else(|| {
                                let video = obj.video()?;
                                obj.client().prepared_audio_playback_uri(&video)
                            });
                            if let Some(uri) = uri {
                                feed_scratch(engine, uri);
                            }
                        }
                        None => log::info!(
                            "Scratch: the record is taken at {:.2} s, without sound (nothing of the track at hand)",
                            session.origin
                        ),
                    }
                }
                session.origin -= degrees / SPIN;
            }
            session.hand = (session.origin + degrees / SPIN).clamp(0.0, session.duration - 0.05);
            if let Some(engine) = &session.engine {
                engine.hand(session.hand);
            }
            let (hand, duration) = (session.hand, session.duration);
            drop(slot);
            self.show_scratch(hand, duration);
        }

        /// While the record is in a hand, the arm, the line and the clock
        /// follow it.
        fn show_scratch(&self, position: f64, duration: f64) {
            let fraction = (position / duration.max(0.001)).clamp(0.0, 1.0);
            self.dj_deck.set_progress(fraction);
            self.dj_seek.set_value(fraction);
            self.dj_elapsed.set_label(&clock(position));
        }

        /// The record is let go.
        fn scratch_up(&self) {
            let obj = self.obj();
            let mut slot = self.scratch.borrow_mut();
            let Some(session) = slot.as_mut().filter(|session| session.taken) else {
                return;
            };
            session.taken = false;
            let Some(player) = self.clapper.player() else {
                return;
            };
            let Some(engine) = &session.engine else {
                // Without sound: the track goes on where the hand left it.
                player.seek(session.hand);
                if session.was_playing {
                    player.play();
                }
                drop(slot);
                self.end_scratch();
                return;
            };
            engine.release(session.was_playing);
            log::info!(
                "Scratch: let go at {:.2} s, the record turning at {:+.2}",
                engine.position(),
                engine.rate()
            );
            let seek_done = std::rc::Rc::new(Cell::new(false));
            let signal = player.connect_seek_done(clone!(
                #[strong]
                seek_done,
                move |_| seek_done.set(true)
            ));
            session.released = Some(Released {
                at: std::time::Instant::now(),
                position: engine.position(),
                turn: self.dj_deck.turn(),
                target: None,
                seek_done,
                seeks: 0,
                handed: None,
                joined: None,
                signal: Some(signal),
            });
            if session.timer.is_none() {
                session.timer = Some(glib::timeout_add_local(
                    std::time::Duration::from_millis(8),
                    clone!(
                        #[weak]
                        obj,
                        #[upgrade_or]
                        glib::ControlFlow::Break,
                        move || obj.imp().scratch_step()
                    ),
                ));
            }
        }

        /// Let go, every few milliseconds: the record on the screen turns
        /// with the needle, and the player takes over when the needle is
        /// where the player waits.
        fn scratch_step(&self) -> glib::ControlFlow {
            let obj = self.obj();
            let mut slot = self.scratch.borrow_mut();
            let (Some(session), Some(player)) = (slot.as_mut(), self.clapper.player()) else {
                return glib::ControlFlow::Break;
            };
            let (Some(released), Some(engine)) = (session.released.as_mut(), &session.engine)
            else {
                session.timer = None;
                return glib::ControlFlow::Break;
            };
            let position = engine.position();
            let (duration, was_playing) = (session.duration, session.was_playing);
            self.dj_deck
                .set_turn(released.turn + (position - released.position) * SPIN);
            let since = released.at.elapsed().as_secs_f64();

            // A record that was not playing comes to rest; the player waits
            // where it stopped.
            if !was_playing {
                if since > 3.0 * crate::gui::scratch::SPIN_UP {
                    player.seek_custom(position, clapper::PlayerSeekMethod::Accurate);
                    drop(slot);
                    self.end_scratch();
                    return glib::ControlFlow::Break;
                }
                drop(slot);
                self.show_scratch(position, duration);
                return glib::ControlFlow::Continue;
            }

            // The player was told to play: the needle goes on until it is
            // heard, then the one fades in as the other fades out.
            if let Some(handed) = released.handed {
                let playing = obj.player_state() == clapper::PlayerState::Playing;
                let waited = handed.elapsed().as_millis();
                let (_, end) = engine.range();
                match released.joined {
                    Some(joined) if joined.elapsed().as_millis() >= HAND_BACK_SETTLE => {
                        drop(slot);
                        self.end_scratch();
                        return glib::ControlFlow::Break;
                    }
                    Some(_) => return glib::ControlFlow::Continue,
                    None if playing => {
                        released.joined = Some(std::time::Instant::now());
                        log::info!(
                            "Scratch: the player has the track again at {:.2} s, {:+.0} ms from the record ({:.2} s after it was let go)",
                            obj.heard_position(),
                            (obj.heard_position() - position) * 1000.0,
                            since
                        );
                        engine.end();
                        drop(slot);
                        self.fade_in();
                        return glib::ControlFlow::Continue;
                    }
                    // The player is slow to start: meet further on instead,
                    // while there is audio to go on with.
                    None if waited > HAND_BACK_PATIENCE
                        && released.seeks < 3
                        && end - position > 2.0 =>
                    {
                        log::info!("Scratch: the player did not start in time; meeting further on");
                        player.pause();
                        released.handed = None;
                        released.target = None;
                    }
                    None if waited > 10_000 => {
                        drop(slot);
                        self.end_scratch();
                        return glib::ControlFlow::Break;
                    }
                    None => {}
                }
                drop(slot);
                self.show_scratch(position, duration);
                return glib::ControlFlow::Continue;
            }

            let give_up = since > 6.0;
            let ready = released.seek_done.get();
            match released.target {
                None => {
                    // Where the needle will be once it turns at (nearly) the
                    // recorded speed, as far as there is audio.
                    let off = engine.rate() - 1.0;
                    // (Twice as far on for every time the player was late.)
                    let lasts = (crate::gui::scratch::SPIN_UP * (off.abs() / 0.03).max(1.0).ln())
                        .clamp(HAND_BACK_FROM, 2.5)
                        .max(HAND_BACK_FROM * f64::from(1u32 << released.seeks.min(3)));
                    let coast = off
                        * crate::gui::scratch::SPIN_UP
                        * (1.0 - (-lasts / crate::gui::scratch::SPIN_UP).exp());
                    let (_, end) = engine.range();
                    let target = (position + lasts + coast)
                        .min(end - 0.2)
                        .max(position)
                        .clamp(0.0, (duration - 0.3).max(0.0));
                    released.seek_done.set(false);
                    released.seeks += 1;
                    released.target = Some(target);
                    player.seek_custom(target, clapper::PlayerSeekMethod::Accurate);
                }
                Some(target) if ready || give_up => {
                    let there = position >= target - HAND_BACK_LEAD;
                    let late = position > target + 0.12;
                    if late && released.seeks < 3 && !engine.starved() && !give_up {
                        // The player took too long: meet further on.
                        released.target = None;
                    } else if there || engine.starved() || give_up {
                        released.handed = Some(std::time::Instant::now());
                        log::info!(
                            "Scratch: handing back at {:.2} s (the player waits at {:.2} s, the record turns at {:+.2}{})",
                            position,
                            target,
                            engine.rate(),
                            if engine.starved() { ", out of audio" } else { "" }
                        );
                        drop(slot);
                        // Silent until it is heard to play (see above).
                        obj.set_fade_gain(Some(0.0));
                        player.play();
                        return glib::ControlFlow::Continue;
                    }
                }
                Some(_) => {}
            }
            drop(slot);
            self.show_scratch(position, duration);
            glib::ControlFlow::Continue
        }

        /// The player plays again: its sound comes in as the needle's goes.
        fn fade_in(&self) {
            let obj = self.obj();
            let step = Cell::new(0u32);
            let ramp = glib::timeout_add_local(
                std::time::Duration::from_millis(8),
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    glib::ControlFlow::Break,
                    move || {
                        const STEPS: u32 = 8;
                        step.set(step.get() + 1);
                        if step.get() >= STEPS {
                            obj.imp().scratch_ramp.take();
                            obj.set_fade_gain(None);
                            return glib::ControlFlow::Break;
                        }
                        obj.set_fade_gain(Some(step.get() as f64 / STEPS as f64));
                        glib::ControlFlow::Continue
                    }
                ),
            );
            if let Some(old) = self.scratch_ramp.replace(Some(ramp)) {
                old.remove();
            }
        }

        /// The scratch is over (handed back, or given up): the record turns
        /// by itself again.
        pub(super) fn end_scratch(&self) {
            let Some(mut session) = self.scratch.take() else {
                return;
            };
            if let Some(timer) = session.timer.take() {
                timer.remove();
            }
            if let Some(released) = session.released.take() {
                released.forget(self.clapper.player().as_ref());
            }
            if let Some(ramp) = self.scratch_ramp.take() {
                ramp.remove();
                self.obj().set_fade_gain(None);
            }
            self.dj_deck.set_held(false);
            if let Some(engine) = session.engine.take() {
                #[cfg(feature = "dev-hooks")]
                if let (Ok(path), Some((samples, rate))) =
                    (std::env::var("KH_SCRATCH_DUMP"), engine.dump())
                {
                    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                    let _ = std::fs::write(&path, bytes);
                    log::info!("DEV: scratch: {} frames at {rate} Hz written to {path}", samples.len() / 2);
                }
                // Its last sound fades; then it goes.
                engine.end();
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    drop(engine)
                });
            }
            self.update_dj_progress();
        }

        /// Kinora: the DJ's line was moved by hand. Seek once the hand rests,
        /// not for every pixel on the way.
        fn dj_seek_to(&self, fraction: f64) {
            let obj = self.obj();
            if let Some(duration) = obj.duration_secs() {
                self.dj_elapsed
                    .set_label(&clock(fraction.clamp(0.0, 1.0) * duration));
            }
            let source = glib::timeout_add_local_once(
                std::time::Duration::from_millis(150),
                clone!(
                    #[weak]
                    obj,
                    move || {
                        let imp = obj.imp();
                        imp.dj_seek_pending.take();
                        imp.dj_seek_quiet_until.set(Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(800),
                        ));
                        if let (Some(player), Some(duration)) =
                            (imp.clapper.player(), obj.duration_secs())
                        {
                            player.seek(imp.dj_seek.value().clamp(0.0, 1.0) * duration);
                        }
                    }
                ),
            );
            if let Some(old) = self.dj_seek_pending.replace(Some(source)) {
                old.remove();
            }
        }

        /// Kinora: the DJ took this page over, or gave it back.
        fn set_dj(&self, dj: bool) {
            if self.dj.replace(dj) == dj {
                return;
            }
            if let Some(tap) = self.tap() {
                tap.set_recording(dj);
            }
            let obj = self.obj();
            if dj {
                self.up_next_box.set_visible(false);
            } else {
                obj.set_dj_panel(None);
                if self.up_next_store.n_items() > 0 {
                    self.up_next_box.set_visible(true);
                } else if let Some(video) = obj.video() {
                    self.load_up_next(&video);
                }
            }
        }

        /// Kinora: Previous and Next on the player, either side of Clapper's
        /// play button (also in fullscreen). Clapper's bar has no place for
        /// them of its own (its buttons only walk its own queue, which holds
        /// one video here), so they are put into its button box. They are the
        /// window's `player-previous` and `player-next`, and are greyed out
        /// with those actions.
        fn add_transport_buttons(&self) {
            fn find(widget: &gtk::Widget, type_name: &str) -> Option<gtk::Widget> {
                if widget.type_().name() == type_name {
                    return Some(widget.clone());
                }
                let mut child = widget.first_child();
                while let Some(c) = child {
                    if let Some(found) = find(&c, type_name) {
                        return Some(found);
                    }
                    child = c.next_sibling();
                }
                None
            }
            let play = find(self.clapper.upcast_ref(), "ClapperGtkSimpleControls")
                .and_then(|controls| find(&controls, "ClapperGtkTogglePlayButton"));
            let Some((play, parent)) = play.and_then(|p| {
                let parent = p.parent().and_downcast::<gtk::Box>()?;
                Some((p, parent))
            }) else {
                log::warn!("Player: no place for Previous and Next in Clapper's controls");
                return;
            };
            let button = |icon: &str, tooltip: String, action: &str| {
                let button = gtk::Button::builder()
                    .icon_name(icon)
                    .tooltip_text(tooltip)
                    .action_name(action)
                    .valign(gtk::Align::Center)
                    .focus_on_click(false)
                    .build();
                // Look like the play button beside them.
                for class in play.css_classes() {
                    button.add_css_class(&class);
                }
                button.add_css_class("k-transport");
                button
            };
            let previous = button(
                "media-skip-backward-symbolic",
                gettextrs::gettext("Previous (Shift+P)"),
                "win.player-previous",
            );
            let next = button(
                "media-skip-forward-symbolic",
                gettextrs::gettext("Next (Shift+N)"),
                "win.player-next",
            );
            parent.insert_child_after(&previous, play.prev_sibling().as_ref());
            parent.insert_child_after(&next, Some(&play));
        }

        /// Kinora: toggle screen (`window_mode == false`) or window fullscreen.
        /// Pressing the other mode's button switches straight over.
        pub(super) fn toggle_fullscreen_mode(&self, window_mode: bool) {
            let obj = self.obj();
            if obj.fullscreened() {
                let same_mode = obj.window_mode() == window_mode;
                obj.set_fullscreened(false);
                if same_mode {
                    return;
                }
            }
            obj.set_window_mode(window_mode);
            obj.set_fullscreened(true);
        }

        /// Kinora: test hooks (feature `dev-hooks`): open the first Up next video
        /// (`KH_NEXT_AFTER=seconds`), scroll to Up next (`KH_SCROLL_AFTER`).
        #[cfg(feature = "dev-hooks")]
        fn dev_hooks_after_up_next(&self) {
            use std::sync::atomic::{AtomicBool, Ordering};
            static NEXT_DONE: AtomicBool = AtomicBool::new(false);
            let obj = self.obj().clone();
            if let Ok(secs) = std::env::var("KH_NEXT_AFTER") {
                if !NEXT_DONE.swap(true, Ordering::SeqCst) {
                    let o = obj.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_secs(secs.parse().unwrap_or(15)),
                        move || {
                            if let Some(v) = o.imp().up_next_store.item(0).and_downcast::<Video>() {
                                log::info!("DEV: opening Up next video {}", v.id().id);
                                o.emit_by_name::<()>("play-video", &[&v, &PlayType::Default]);
                            }
                        },
                    );
                }
            }
            if let Ok(secs) = std::env::var("KH_SCROLL_AFTER") {
                let o = obj.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_secs(secs.parse().unwrap_or(10)),
                    move || {
                        let b = &o.imp().up_next_box;
                        let sw = &o.imp().page_scroller;
                        let y = b
                            .compute_point(&**sw, &gtk::graphene::Point::new(0.0, 0.0))
                            .map(|p| p.y())
                            .unwrap_or(0.0);
                        let adj = sw.vadjustment();
                        adj.set_value(adj.value() + y as f64 - 40.0);
                    },
                );
            }
        }

        /// Kinora: back to the top; again shortly after, because focus moving into
        /// the page during its transition scrolls it to the focused widget.
        fn scroll_to_top(&self) {
            self.scroll_to_top_on_show.set(false);
            for delay in [0, 450] {
                let scroller = self.page_scroller.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                    scroller.vadjustment().set_value(0.0);
                });
            }
        }

        /// When the video has played to its end, continue with the first "Up next" video.
        fn maybe_autoplay(&self, player: &clapper::Player) {
            let obj = self.obj();
            if player.state() == clapper::PlayerState::Playing {
                obj.set_loading(false);
                if let Some(started) = self.play_started.take() {
                    log::info!(
                        "Player: playing after {:.1} s",
                        started.elapsed().as_secs_f64()
                    );
                }
            }
            log::debug!(
                "autoplay check: state {:?}, position {:.2}, duration {:?}, up next {}",
                player.state(),
                player.position(),
                player
                    .queue()
                    .and_then(|q| q.current_item())
                    .map(|i| i.duration()),
                self.up_next_store.n_items()
            );
            // Live streams have no real end; their position runs with the duration.
            // (Nor does a DJ track: the DJ moves on by itself.)
            if self.autoplay_done.get()
                || self.dj.get()
                || obj.video().is_some_and(|v| v.is_live())
                || obj.external()
                || !self.settings.boolean("autoplay-next")
                || !matches!(
                    player.state(),
                    clapper::PlayerState::Stopped | clapper::PlayerState::Paused
                )
            {
                return;
            }
            let duration = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
                .unwrap_or(0.0);
            // Only at the very end, not on an ordinary pause.
            if duration <= 0.0 || player.position() < duration - 0.75 {
                return;
            }
            let Some(next) = self
                .up_next_store
                .item(0)
                .and_then(|o| o.downcast::<Video>().ok())
            else {
                return;
            };
            self.autoplay_done.set(true);
            obj.emit_by_name::<()>("play-video", &[&next, &PlayType::Default]);
        }

        fn get_state(&self) -> clapper::PlayerState {
            // (The template's own bindings ask before the player is there.)
            self.clapper
                .try_get()
                .and_then(|clapper| clapper.player())
                .map_or(clapper::PlayerState::Stopped, |p| p.state())
        }

        fn get_position(&self) -> f64 {
            self.clapper
                .player()
                .expect("Clapper to have a player")
                .position()
        }

        fn get_duration(&self) -> f64 {
            self.clapper
                .player()
                .expect("Clapper to have a player")
                .queue()
                .expect("Player to have a queue")
                .current_item()
                .expect("Queue to have a current item")
                .duration()
        }
    }

    #[gtk::template_callbacks]
    impl VideoPage {
        #[template_callback(function)]
        fn format_uploaded(uploaded: String) -> String {
            // Kinora: nothing until the upload date is known.
            if uploaded.is_empty() {
                return String::new();
            }
            gettextrs::gettext("Uploaded {}").replace("{}", &uploaded)
        }

        /// Kinora: "Start DJ" is for YouTube videos, while the DJ is not on.
        #[template_callback(function)]
        fn dj_offered(available: bool, dj: bool, platform: Option<String>) -> bool {
            available && !dj && platform.as_deref() == Some("youtube")
        }

        // Kinora: what the DJ's page shows instead of the video page (see
        // docs/design.md, "DJ"). In fullscreen a track with its video is the
        // video alone; the record player keeps its page.
        #[template_callback(function)]
        fn dj_vinyl(dj: bool, audio_only: bool) -> bool {
            dj && audio_only
        }

        #[template_callback(function)]
        fn stage_visible(dj: bool, audio_only: bool) -> bool {
            !(dj && audio_only)
        }

        #[template_callback(function)]
        fn stage_light_visible(dj: bool, fullscreened: bool) -> bool {
            !dj && !fullscreened
        }

        #[template_callback(function)]
        fn dj_chrome_visible(dj: bool, audio_only: bool, fullscreened: bool) -> bool {
            dj && (audio_only || !fullscreened)
        }

        #[template_callback(function)]
        fn details_visible(dj: bool, fullscreened: bool) -> bool {
            !dj && !fullscreened
        }

        #[template_callback(function)]
        fn is_playing(state: clapper::PlayerState) -> bool {
            state == clapper::PlayerState::Playing
        }

        #[template_callback(function)]
        fn dj_play_icon(state: clapper::PlayerState, loading: bool) -> &'static str {
            match state {
                clapper::PlayerState::Playing => "media-playback-pause-symbolic",
                _ if loading => "content-loading-symbolic",
                _ => "media-playback-start-symbolic",
            }
        }

        #[template_callback(function)]
        fn description_is_long(description: Option<String>) -> bool {
            description.is_some_and(|d| d.chars().count() > 280 || d.lines().count() > 4)
        }

        /// The first few lines of a long description, unless expanded. (GTK's
        /// line limit doesn't apply to this selectable, linkified label.)
        #[template_callback(function)]
        fn collapse_description(description: Option<String>, expanded: bool) -> Option<String> {
            let d = description?;
            if expanded || !Self::description_is_long(Some(d.clone())) {
                return Some(d);
            }
            let mut short: String = d.lines().take(4).collect::<Vec<_>>().join("\n");
            if short.chars().count() > 280 {
                short = short.chars().take(280).collect();
            }
            Some(format!("{}…", short.trim_end()))
        }

        #[template_callback(function)]
        fn description_toggle_label(expanded: bool) -> String {
            if expanded {
                gettextrs::gettext("Show less")
            } else {
                gettextrs::gettext("Show more")
            }
        }

        #[template_callback]
        fn toggle_description(&self) {
            let obj = self.obj();
            obj.set_description_expanded(!obj.description_expanded());
        }

        #[template_callback]
        fn up_next_row_activated(&self, row: &gtk::ListBoxRow) {
            if let Some(video) = self
                .up_next_store
                .item(row.index() as u32)
                .and_then(|o| o.downcast::<Video>().ok())
            {
                self.obj()
                    .emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
            }
        }

        #[template_callback]
        fn display_channel(&self) {
            let obj = self.obj();
            if let Some(channel) = obj.video().map(|v| v.uploader()) {
                obj.emit_by_name::<()>("display-channel", &[&channel]);
            }
        }

        #[template_callback]
        fn toggle_fullscreen(&self) {
            // Kinora: a double click on the left or right third of the player
            // seeks 10 seconds instead (Clapper reports every double click here).
            if let Some((side, at)) = self.double_click.take()
                && side != 0
                && at.elapsed() < std::time::Duration::from_millis(700)
            {
                let action = if side > 0 {
                    "video.seek-forward"
                } else {
                    "video.seek-backward"
                };
                let _ = WidgetExt::activate_action(&*self.obj(), action, None);
                return;
            }
            self.toggle_fullscreen_mode(false);
        }

        /// Kinora: the video fills the window, not the screen.
        #[template_callback]
        fn toggle_window_fullscreen(&self) {
            self.toggle_fullscreen_mode(true);
        }

        #[template_callback(function)]
        fn window_fullscreen_icon(fullscreened: bool, window_mode: bool) -> &'static str {
            if fullscreened && window_mode {
                "window-restore-symbolic"
            } else {
                "window-maximize-symbolic"
            }
        }

        #[template_callback(function)]
        fn window_fullscreen_tooltip(fullscreened: bool, window_mode: bool) -> String {
            if fullscreened && window_mode {
                gettextrs::gettext("Exit Window Fullscreen (T)")
            } else {
                gettextrs::gettext("Window Fullscreen (T)")
            }
        }

        #[template_callback]
        fn download(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("download-video", &[&video]);
            }
        }

        #[template_callback]
        fn remove_downloaded(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("remove-downloaded", &[&video]);
            }
        }

        #[template_callback]
        fn play_external(&self) {
            let obj = self.obj();
            obj.play_external();
        }

        #[template_callback]
        fn copy_to_clipboard(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("copy-video-url-to-clipboard", &[&video]);
            }
        }

        fn setup_actions(&self) {
            let obj = self.obj();

            let action_play_pause = SimpleAction::new("play-pause", None);
            action_play_pause.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.play_pause();
                }
            ));
            let action_seek_forward = SimpleAction::new("seek-forward", None);
            action_seek_forward.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    // TODO: Maybe configurable duration.
                    obj.seek_by(10.0);
                    obj.imp()
                        .billboard
                        .post_message("media-seek-forward", &gettextrs::gettext("10 Seconds"));
                }
            ));

            let action_seek_backward = SimpleAction::new("seek-backward", None);
            action_seek_backward.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    // TODO: Maybe configurable duration.
                    obj.seek_by(-10.0);
                    obj.imp()
                        .billboard
                        .post_message("media-seek-backward", &gettextrs::gettext("10 Seconds"));
                }
            ));

            let action_seek_percentile =
                SimpleAction::new("seek-percentile", Some(&i32::static_variant_type()));
            action_seek_percentile.connect_activate(clone!(
                #[weak]
                obj,
                move |_, parameter| {
                    let parameter: i32 = parameter.unwrap().get().unwrap();
                    obj.seek_to_percent((parameter as f64) / 10.0)
                }
            ));

            self.clapper.connect_seek_request(clone!(
                #[weak]
                obj,
                move |_, forward| {
                    // TODO: Maybe configurable duration.
                    if forward {
                        obj.seek_by(10.);
                        obj.imp()
                            .billboard
                            .post_message("media-seek-forward", &gettextrs::gettext("10 Seconds"));
                    } else {
                        obj.seek_by(-10.);
                        obj.imp()
                            .billboard
                            .post_message("media-seek-backward", &gettextrs::gettext("10 Seconds"));
                    }
                }
            ));

            // Kinora: for the DJ's bar, which has no player to double click.
            let action_fullscreen = SimpleAction::new("fullscreen", None);
            action_fullscreen.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| obj.imp().toggle_fullscreen_mode(false)
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("video", Some(&actions));
            actions.add_action(&action_fullscreen);
            actions.add_action(&action_play_pause);
            actions.add_action(&action_seek_forward);
            actions.add_action(&action_seek_backward);
            actions.add_action(&action_seek_percentile);

            let clapper_actions = [
                "speed-up",
                "speed-down",
                "volume-up",
                "volume-down",
                "toggle-mute",
            ];
            let clapper = self.clapper.get();
            for action_name in clapper_actions {
                let action = SimpleAction::new(action_name, None);
                action.connect_activate(clone!(
                    #[weak]
                    clapper,
                    move |_, _| {
                        let _ = clapper.activate_action(&format!("video.{action_name}"), None);
                    }
                ));
                actions.add_action(&action);
            }
        }

        fn setup(&self) {
            let obj = self.obj();
            let settings = &self.settings;
            let billboard = &self.billboard;

            // Kinora: the deck's audio (the scratch and beat matching).
            let tap = self.clapper.player().and_then(|player| {
                let tap = crate::gui::deck_audio::Tap::new()?;
                player.set_audio_filter(Some(tap.element()));
                tap.set_recording(self.dj.get());
                Some(tap)
            });
            let _ = self.tap.set(tap);

            let shortcut_escape = Shortcut::new(
                Some(
                    ShortcutTrigger::parse_string("Escape")
                        .expect("Escape to be a valid shortcut trigger"),
                ),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        if obj.fullscreened() {
                            obj.set_fullscreened(false);
                        } else {
                            // Kinora: Esc shrinks the page into the mini-player.
                            let _ = WidgetExt::activate_action(&obj, "win.minimize-player", None);
                        }
                        Propagation::Stop
                    }
                ))),
            );
            let shortcut_fullscreen = Shortcut::new(
                Some(
                    ShortcutTrigger::parse_string("F|F11")
                        .expect("F|F11 to be a valid shortcut trigger"),
                ),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        obj.imp().toggle_fullscreen();
                        Propagation::Stop
                    }
                ))),
            );

            // Kinora: "T" fills the window, like YouTube's theater mode.
            let shortcut_window_fullscreen = Shortcut::new(
                Some(ShortcutTrigger::parse_string("T").expect("T to be a valid shortcut trigger")),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        obj.imp().toggle_fullscreen_mode(true);
                        Propagation::Stop
                    }
                ))),
            );

            let shortcut_controller = ShortcutController::new();
            shortcut_controller.add_shortcut(shortcut_escape);
            shortcut_controller.add_shortcut(shortcut_fullscreen);
            shortcut_controller.add_shortcut(shortcut_window_fullscreen);
            obj.add_controller(shortcut_controller);

            // Kinora: remember where the player is double-clicked (see
            // `toggle_fullscreen`). Clicks on its controls don't count.
            let double_click = gtk::GestureClick::new();
            double_click.set_propagation_phase(gtk::PropagationPhase::Capture);
            double_click.connect_pressed(clone!(
                #[weak]
                obj,
                move |gesture, n_press, x, y| {
                    let imp = obj.imp();
                    let clapper = gesture.widget().expect("Gesture to have a widget");
                    let mut target = clapper.pick(x, y, gtk::PickFlags::DEFAULT);
                    let mut on_control = false;
                    while let Some(widget) = target {
                        if widget == clapper {
                            break;
                        }
                        on_control |= widget.is::<gtk::Button>()
                            || widget.is::<gtk::Range>()
                            || widget.type_().name() == "ClapperGtkSimpleControls";
                        target = widget.parent();
                    }
                    if n_press != 2 || on_control {
                        imp.double_click.set(None);
                        return;
                    }
                    let third = clapper.width() as f64 / 3.0;
                    let side = if x < third {
                        -1
                    } else if x > 2.0 * third {
                        1
                    } else {
                        0
                    };
                    imp.double_click
                        .set(Some((side, std::time::Instant::now())));
                }
            ));
            self.clapper.add_controller(double_click);

            // Kinora: show the buffered part ahead of the playhead.
            crate::gui::buffer_bar::attach(&self.clapper);
            self.add_transport_buttons();
            self.up_next_store.connect_items_changed(clone!(
                #[weak]
                obj,
                move |store, _, _, _| {
                    let has_next = store.n_items() > 0;
                    if obj.imp().has_next.replace(has_next) != has_next {
                        obj.notify_has_next();
                    }
                }
            ));

            // Kinora: no MPRIS player per page: the window has one for the
            // app (see `gui/mpris.rs`).
            let player = self.clapper.player().expect("Clapper to have a Player");
            if let Some(mpris) = player
                .enhancer_proxies()
                .and_then(|p| p.proxy_by_module("clapper-mpris"))
            {
                mpris.set_target_creation_allowed(false);
            }

            player.connect_state_notify(clone!(
                #[weak]
                settings,
                move |p| {
                    let _ =
                        settings.set_uint("player-adaptive-start-bitrate", p.adaptive_bandwidth());
                }
            ));

            // Kinora: the player lives in the page's scrolling column, where it
            // gets no height of its own; size it 16:9, as large as the page allows
            // (at most 72% of its height), centred on the stage with the glow
            // around it. Narrow windows use every pixel: the player runs edge to
            // edge. Fullscreen lifts all of this and lets it fill.
            // Wide windows also put Up next in a column beside the details.
            self.stage.set_measure_overlay(&*self.stage_clamp, true);
            // (The DJ's page: a track's video is as high as the record player
            // would be, see `fit`.)
            let last = Cell::new((0, 0, false, false));
            obj.add_tick_callback(move |obj, _| {
                    let state = (obj.width(), obj.height(), obj.fullscreened(), obj.dj());
                    // Nothing to do unless the page changed size.
                    if last.replace(state) == state {
                        return glib::ControlFlow::Continue;
                    }
                    let imp = obj.imp();
                    let clapper = imp.clapper.get();
                    let (width, height, fullscreened, dj) = state;
                    if dj {
                        imp.stage.add_css_class("dj");
                    } else {
                        imp.stage.remove_css_class("dj");
                    }
                    if fullscreened {
                        clapper.set_height_request(-1);
                        imp.stage_clamp.set_maximum_size(i32::MAX);
                        imp.stage.add_css_class("fullscreen");
                        return glib::ControlFlow::Continue;
                    }
                    imp.stage.remove_css_class("fullscreen");
                    let narrow = width < 640;
                    if narrow {
                        imp.stage.add_css_class("edge");
                    } else {
                        imp.stage.remove_css_class("edge");
                    }
                    let gutter = if narrow { 0.0 } else { 24.0 };
                    let by_width = (width as f64 - 2.0 * gutter) * 9.0 / 16.0;
                    let cap = height as f64 * if dj { 0.44 } else { 0.72 };
                    let player_height = by_width.min(cap).max(120.0).round();
                    clapper.set_height_request(player_height as i32);
                    imp.stage_clamp
                        .set_maximum_size((player_height * 16.0 / 9.0).round() as i32);

                    let two_columns = width >= 1080;
                    imp.body.set_orientation(if two_columns {
                        gtk::Orientation::Horizontal
                    } else {
                        gtk::Orientation::Vertical
                    });
                    imp.body_clamp
                        .set_maximum_size(if two_columns { 1360 } else { 850 });
                    let column = if two_columns { 400 } else { 10000 };
                    imp.up_next_clamp.set_maximum_size(column);
                    imp.up_next_clamp.set_tightening_threshold(column);
                    imp.up_next_clamp.set_hexpand(!two_columns);
                    glib::ControlFlow::Continue
            });

            // Kinora: the DJ's page. Its line seeks; a click on the deck plays
            // or pauses, and the record can be scratched; nothing turns by
            // itself with "Reduce effects".
            self.dj_seek.connect_change_value(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                Propagation::Proceed,
                move |_, _, value| {
                    obj.imp().dj_seek_to(value);
                    Propagation::Proceed
                }
            ));
            self.dj_deck.connect_hand(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                false,
                move |_, hand| obj.imp().deck_hand(hand)
            ));
            settings
                .bind("reduce-effects", &*self.dj_deck, "still")
                .flags(gtk::gio::SettingsBindFlags::GET)
                .build();
            obj.connect_position_notify(|obj| obj.imp().update_dj_progress());
            obj.connect_duration_notify(|obj| obj.imp().update_dj_progress());

            // Kinora: a new video's page opens at the top once it is shown (pages
            // are laid out only when on screen).
            self.page_scroller.connect_map(clone!(
                #[weak]
                obj,
                move |_| {
                    if obj.imp().scroll_to_top_on_show.get() {
                        obj.imp().scroll_to_top();
                    }
                }
            ));

            // Kinora: if a fast-start stream fails to play, retry with yt-dlp.
            // The handler must be droppable on any thread: Clapper's player
            // can let go of it on its own thread, and a `connect_local`
            // closure dropped there aborts the app ("Value dropped on a
            // different thread", seen on Windows). The page is only reached
            // when the signal comes, on the main thread.
            let page: glib::SendWeakRef<super::VideoPage> = obj.downgrade().into();
            player.connect("error", false, move |args| {
                let obj = page.upgrade()?;
                let imp = obj.imp();
                if !imp.fast_start_active.replace(false) {
                    obj.set_loading(false);
                    return None;
                }
                let error = args
                    .get(1)
                    .and_then(|v| v.get::<glib::Error>().ok())
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                log::warn!("Fast start stream failed ({error}); retrying with yt-dlp");
                if let (Some(video), Some(player)) = (obj.video(), imp.clapper.player()) {
                    let queue = player.queue().expect("Clapper does not have a queue");
                    queue.clear();
                    queue.add_item(&super::titled_item(
                        &obj.convert_to_clapper_playable_url(&video),
                        &video,
                    ));
                    obj.start_player(&player);
                }
                None
            });

            // Kinora: "Up next" list and autoplay.
            self.up_next_list.bind_model(
                Some(&self.up_next_store),
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |item| {
                        let video = item
                            .downcast_ref::<Video>()
                            .expect("Up next items to be videos");
                        let video_item = VideoItem::new();
                        video_item.set_client(obj.client());
                        video_item.set_compact(true);
                        video_item.set_video(video);
                        // The item's own menu ("play externally", ...).
                        video_item.connect_local(
                            "play-video",
                            false,
                            clone!(
                                #[weak]
                                obj,
                                #[upgrade_or_default]
                                move |args| {
                                    let video: Video = args[1].get().expect("Video argument");
                                    let play_type: PlayType =
                                        args[2].get().expect("PlayType argument");
                                    obj.emit_by_name::<()>("play-video", &[&video, &play_type]);
                                    None
                                }
                            ),
                        );
                        gtk::ListBoxRow::builder()
                            .child(&video_item)
                            .build()
                            .upcast()
                    }
                ),
            );
            settings
                .bind("autoplay-next", &*self.autoplay_switch, "active")
                .build();
            player.connect_state_notify(clone!(
                #[weak]
                obj,
                move |player| {
                    obj.imp().maybe_autoplay(player);
                }
            ));

            // Set up player settings
            settings.bind("player-volume", &player, "volume").build();
            settings.bind("player-speed", &player, "speed").build();
            settings
                .bind("player-subtitles-enabled", &player, "subtitles-enabled")
                .build();

            // Display billboard upon volume or speed change
            player.connect_volume_notify(clone!(
                #[weak]
                billboard,
                #[weak]
                obj,
                move |_| {
                    // Not while the DJ fades it, nor just after.
                    let imp = obj.imp();
                    let quiet = imp
                        .quiet_volume_until
                        .get()
                        .is_some_and(|until| std::time::Instant::now() < until);
                    if imp.fade_base.get().is_none() && !quiet {
                        billboard.announce_volume();
                    }
                }
            ));

            player.connect_speed_notify(clone!(
                #[weak]
                billboard,
                move |_| {
                    billboard.announce_speed();
                }
            ));

            player.connect_state_notify(clone!(
                #[weak]
                obj,
                move |_player| {
                    obj.notify_state();
                }
            ));

            player.connect_position_notify(clone!(
                #[weak]
                obj,
                move |_player| {
                    obj.notify_position();
                }
            ));

            let watch = player
                .property_expression("queue")
                .chain_property::<clapper::Queue>("current-item")
                .chain_property::<clapper::MediaItem>("duration")
                .watch(
                    Some(&player),
                    clone!(
                        #[weak]
                        obj,
                        move || {
                            obj.notify_duration();
                        }
                    ),
                );

            self.watch.replace(Some(watch));

            // Set initial bit rate from last observed bit rate.
            // Very crude approximation, but is simple to implement.
            // A more involved implementation could look at the history of bitrates.
            settings
                .bind(
                    "player-adaptive-start-bitrate",
                    &player,
                    "adaptive-start-bitrate",
                )
                .build();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoPage {
        const NAME: &'static str = "TFVideoPage";
        type Type = super::VideoPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            clapper_gtk::Video::ensure_type();
            clapper_gtk::Billboard::ensure_type();
            Turntable::ensure_type();
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for VideoPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
            self.setup_actions();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: std::sync::LazyLock<Vec<Signal>> = std::sync::LazyLock::new(|| {
                vec![
                    Signal::builder("display-channel")
                        .param_types([Channel::static_type()])
                        .build(),
                    Signal::builder("download-video")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("remove-downloaded")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("copy-video-url-to-clipboard")
                        .param_types([Video::static_type()])
                        .build(),
                    // Kinora: play a video from "Up next" (or autoplay).
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }

        fn dispose(&self) {
            if let Some(watch) = self.watch.borrow().as_ref() {
                watch.unwatch();
            }
        }
    }

    impl WidgetImpl for VideoPage {
        fn grab_focus(&self) -> bool {
            // (The DJ's record player: the player itself is not shown.)
            if self.vinyl() {
                return self.dj_play.grab_focus();
            }
            self.clapper.grab_focus()
        }
    }
    impl BinImpl for VideoPage {}
}
