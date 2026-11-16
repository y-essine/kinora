//! Kinora: the media player integration (MPRIS), one for the whole app.
//!
//! Clapper's own MPRIS enhancer is off: it registered one player per video
//! page (the DJ keeps two or three around), named the track after the stream
//! (`….audio.m3u8` for prepared audio) and sent Next and Previous to
//! Clapper's queue, which in Kinora only ever holds one item. This one
//! follows the player sheet's page, takes title, channel and artwork from
//! the video, and sends Next and Previous to the same `win.player-next` and
//! `win.player-previous` actions as the buttons (so the DJ crossfades).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib::{self, Variant, prelude::ToVariant};
use gtk::prelude::*;

use crate::gui::video_page::VideoPage;
use crate::gui::window::Window;

const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

const XML: &str = r#"<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="DesktopEntry" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek"><arg direction="in" name="Offset" type="x"/></method>
    <method name="SetPosition">
      <arg direction="in" name="TrackId" type="o"/>
      <arg direction="in" name="Position" type="x"/>
    </method>
    <method name="OpenUri"><arg direction="in" name="Uri" type="s"/></method>
    <signal name="Seeked"><arg name="Position" type="x"/></signal>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="Rate" type="d" access="readwrite"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read"/>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
</node>"#;

/// The properties that change while playing (sent with PropertiesChanged).
const LIVE: [&str; 7] = [
    "PlaybackStatus",
    "Metadata",
    "CanGoNext",
    "CanGoPrevious",
    "CanPlay",
    "CanPause",
    "CanSeek",
];

#[derive(Default)]
struct Inner {
    window: glib::WeakRef<Window>,
    connection: RefCell<Option<gio::DBusConnection>>,
    page: RefCell<Option<VideoPage>>,
    handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    sent: RefCell<HashMap<&'static str, Variant>>,
    can_previous: Cell<bool>,
    can_next: Cell<bool>,
}

#[derive(Clone, Default)]
pub struct Mpris(Rc<Inner>);

impl Mpris {
    pub fn new(window: &Window) -> Self {
        let mpris = Self(Rc::new(Inner {
            window: window.downgrade(),
            ..Default::default()
        }));
        // `org.mpris.MediaPlayer2.kinora` (`….kinora.Devel` for dev builds,
        // so they can run next to an installed Kinora).
        let name = match crate::config::PROFILE {
            "Devel" | "development" => "org.mpris.MediaPlayer2.kinora.Devel",
            _ => "org.mpris.MediaPlayer2.kinora",
        };
        let on_bus = mpris.clone();
        gio::bus_own_name(
            gio::BusType::Session,
            name,
            gio::BusNameOwnerFlags::NONE,
            move |connection, _| on_bus.register(connection),
            |_, name| log::info!("MPRIS: {name}"),
            |_, name| log::warn!("MPRIS: could not own {name}"),
        );
        mpris
    }

    fn register(&self, connection: gio::DBusConnection) {
        let node = match gio::DBusNodeInfo::for_xml(XML) {
            Ok(node) => node,
            Err(e) => return log::error!("MPRIS: {e}"),
        };
        for interface in [ROOT, PLAYER] {
            let Some(info) = node.lookup_interface(interface) else {
                continue;
            };
            let (calls, props, sets) = (self.clone(), self.clone(), self.clone());
            let registered = connection
                .register_object(PATH, &info)
                .method_call(move |_, _, _, _, method, params, invocation| {
                    calls.call(method, &params);
                    invocation.return_value(None);
                })
                .property(move |_, _, _, _, name| props.property(name))
                .set_property(move |_, _, _, _, name, value| sets.set(name, &value))
                .build();
            if let Err(e) = registered {
                log::error!("MPRIS: {e}");
            }
        }
        self.0.connection.replace(Some(connection));
        self.changed();
    }

    /// Follow `page` (the player sheet's), or nothing.
    pub fn follow(&self, page: Option<VideoPage>) {
        let inner = &self.0;
        if *inner.page.borrow() == page {
            return;
        }
        for (object, handler) in inner.handlers.take() {
            object.disconnect(handler);
        }
        if let Some(page) = &page {
            let mut handlers = Vec::new();
            let weak = Rc::downgrade(&self.0);
            let changed = move || {
                if let Some(inner) = weak.upgrade() {
                    Mpris(inner).changed();
                }
            };
            if let Some(player) = page.clapper_player() {
                let c = changed.clone();
                let id = player.connect_state_notify(move |_| c());
                handlers.push((player.clone().upcast(), id));
                if let Some(queue) = player.queue() {
                    let c = changed.clone();
                    let id = queue.connect_current_item_notify(move |_| c());
                    handlers.push((queue.upcast(), id));
                }
            }
            let c = changed.clone();
            let id = page.connect_video_notify(move |_| c());
            handlers.push((page.clone().upcast(), id));
            // The length arrives once the stream is loaded.
            let c = changed;
            let id = page.connect_position_notify(move |p| {
                if p.duration_secs().is_some() {
                    c()
                }
            });
            handlers.push((page.clone().upcast(), id));
            inner.handlers.replace(handlers);
        }
        inner.page.replace(page);
        self.changed();
    }

    /// Whether Previous and Next go anywhere (see `Window::update_transport`).
    pub fn set_transport(&self, previous: bool, next: bool) {
        self.0.can_previous.set(previous);
        self.0.can_next.set(next);
        self.changed();
    }

    fn page(&self) -> Option<VideoPage> {
        self.0.page.borrow().clone()
    }

    fn call(&self, method: &str, params: &Variant) {
        let window = self.0.window.upgrade();
        let page = self.page();
        let player = page.as_ref().and_then(|p| p.clapper_player());
        match method {
            "Raise" => {
                if let Some(window) = window {
                    window.present();
                }
            }
            "Quit" => {
                if let Some(window) = window {
                    window.close();
                }
            }
            "Next" | "Previous" => {
                if let Some(window) = window {
                    let action = if method == "Next" {
                        "win.player-next"
                    } else {
                        "win.player-previous"
                    };
                    let _ = WidgetExt::activate_action(&window, action, None);
                }
            }
            "PlayPause" => {
                if let Some(page) = page {
                    page.play_pause();
                }
            }
            "Play" => {
                if let Some(player) = player {
                    player.play();
                }
            }
            "Pause" | "Stop" => {
                if let Some(player) = player {
                    player.pause();
                }
            }
            "Seek" => {
                if let (Some(player), Some((offset,))) = (player, params.get::<(i64,)>()) {
                    player.seek((player.position() + offset as f64 / 1e6).max(0.0));
                }
            }
            "SetPosition" => {
                if let (Some(player), Some((_, position))) =
                    (player, params.get::<(glib::variant::ObjectPath, i64)>())
                {
                    player.seek(position as f64 / 1e6);
                }
            }
            _ => {}
        }
    }

    fn property(&self, name: &str) -> Variant {
        let page = self.page();
        let player = page.as_ref().and_then(|p| p.clapper_player());
        let has_page = page.is_some();
        match name {
            "CanQuit" | "CanRaise" | "CanControl" => true.to_variant(),
            "HasTrackList" => false.to_variant(),
            "Identity" => "Kinora".to_variant(),
            "DesktopEntry" => crate::config::APP_ID.to_variant(),
            "SupportedUriSchemes" | "SupportedMimeTypes" => Vec::<String>::new().to_variant(),
            "PlaybackStatus" => match player.map(|p| p.state()) {
                Some(clapper::PlayerState::Playing) => "Playing",
                Some(clapper::PlayerState::Paused | clapper::PlayerState::Buffering) => "Paused",
                _ => "Stopped",
            }
            .to_variant(),
            "Rate" | "MinimumRate" | "MaximumRate" => 1.0f64.to_variant(),
            "Volume" => player.map(|p| p.volume()).unwrap_or(1.0).to_variant(),
            "Position" => player
                .map(|p| (p.position() * 1e6) as i64)
                .unwrap_or_default()
                .to_variant(),
            "Metadata" => self.metadata(page.as_ref()),
            "CanGoNext" => self.0.can_next.get().to_variant(),
            "CanGoPrevious" => self.0.can_previous.get().to_variant(),
            "CanPlay" | "CanPause" | "CanSeek" => has_page.to_variant(),
            _ => Variant::from_none(&glib::VariantTy::VARIANT),
        }
    }

    fn set(&self, name: &str, value: &Variant) -> bool {
        let player = self.page().and_then(|p| p.clapper_player());
        match (name, player, value.get::<f64>()) {
            ("Volume", Some(player), Some(volume)) => {
                player.set_volume(volume.clamp(0.0, 1.0));
                true
            }
            _ => false,
        }
    }

    fn metadata(&self, page: Option<&VideoPage>) -> Variant {
        let dict = glib::VariantDict::new(None);
        let Some(video) = page.and_then(|p| p.video()) else {
            dict.insert_value(
                "mpris:trackid",
                &glib::variant::ObjectPath::try_from("/org/mpris/MediaPlayer2/TrackList/NoTrack")
                    .expect("A valid path")
                    .to_variant(),
            );
            return dict.end();
        };
        let id: String = video
            .id()
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        if let Ok(path) = glib::variant::ObjectPath::try_from(format!(
            "/io/github/y_essine/Kinora/track/v{id}"
        )) {
            dict.insert_value("mpris:trackid", &path.to_variant());
        }
        dict.insert_value("xesam:title", &video.title().to_variant());
        dict.insert_value("xesam:artist", &vec![video.uploader().name()].to_variant());
        if let Some(art) = video.thumbnail_url() {
            dict.insert_value("mpris:artUrl", &art.as_str().to_variant());
        }
        if let Some(length) = page.and_then(|p| p.duration_secs()) {
            dict.insert_value("mpris:length", &((length * 1e6) as i64).to_variant());
        }
        dict.end()
    }

    /// Tell listeners what changed since the last time.
    fn changed(&self) {
        let Some(connection) = self.0.connection.borrow().clone() else {
            return;
        };
        let changed = glib::VariantDict::new(None);
        let mut any = false;
        {
            let mut sent = self.0.sent.borrow_mut();
            for name in LIVE {
                let value = self.property(name);
                if sent.get(name) != Some(&value) {
                    changed.insert_value(name, &value);
                    sent.insert(name, value);
                    any = true;
                }
            }
        }
        if !any {
            return;
        }
        let params = (PLAYER, changed.end(), Vec::<String>::new()).to_variant();
        if let Err(e) = connection.emit_signal(
            None,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&params),
        ) {
            log::warn!("MPRIS: {e}");
        }
    }
}
