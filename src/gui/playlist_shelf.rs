//! Kinora: playlists and mixes on Home (see `docs/design.md`, "DJ").
//!
//! A playlist is not a video, and its card says so: the poster lies on a
//! small stack, a badge counts what is in it, and a kicker names it (Mix,
//! Playlist, Album). Opening one hands it to the DJ, which plays it through
//! and fades from one item into the next.
//!
//! [`PlaylistShelf`] is a titled row of such cards, like `TFVideoRow` is for
//! videos. It takes plain data ([`PlaylistInfo`]) and hides while it is empty.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use url::Url;

use crate::backend::Client;
use crate::gspawn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistKind {
    /// Endless: YouTube makes it up as it plays.
    Mix,
    Playlist,
    Album,
}

/// What a card shows, and what the DJ needs to play it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    /// Who made it (a channel, an artist), if known.
    pub owner: Option<String>,
    /// How many videos it holds; mixes have no end.
    pub count: Option<u32>,
    pub kind: PlaylistKind,
    pub thumbnail: Option<Url>,
}

impl PlaylistInfo {
    fn kicker(&self) -> String {
        match self.kind {
            PlaylistKind::Mix => gettextrs::gettext("Mix"),
            PlaylistKind::Playlist => gettextrs::gettext("Playlist"),
            PlaylistKind::Album => gettextrs::gettext("Album"),
        }
    }

    /// The badge on the poster: "50", or nothing for a mix (it has no end).
    fn badge(&self) -> Option<String> {
        match (self.kind, self.count) {
            (PlaylistKind::Mix, _) | (_, None) => None,
            (_, Some(count)) => Some(count.to_string()),
        }
    }

    fn describe(&self) -> String {
        let mut text = format!("{}: {}", self.kicker(), self.title);
        if let Some(owner) = &self.owner {
            text.push_str(&format!(", {owner}"));
        }
        if let Some(count) = self.badge() {
            text.push_str(&format!(", {count}"));
        }
        text
    }
}

type OpenHandler = Rc<RefCell<Option<Box<dyn Fn(&PlaylistInfo)>>>>;

/// A titled, horizontally scrolling row of playlist cards.
pub struct PlaylistShelf {
    root: gtk::Box,
    cards: gtk::Box,
    client: Client,
    card_width: Cell<i32>,
    items: RefCell<Vec<PlaylistInfo>>,
    on_open: OpenHandler,
}

impl PlaylistShelf {
    pub fn new(title: &str, client: &Client, card_width: i32) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["k-row"])
            .accessible_role(gtk::AccessibleRole::Group)
            .visible(false)
            .build();
        let header = gtk::Box::builder().css_classes(["k-row-header"]).build();
        header.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .valign(gtk::Align::Center)
                .accessible_role(gtk::AccessibleRole::Heading)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["k-row-title"])
                .build(),
        );
        let cards = gtk::Box::builder()
            .spacing(16)
            .css_classes(["k-playlist-cards"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&cards)
            .build();
        root.append(&header);
        root.append(&scroller);
        Self {
            root,
            cards,
            client: client.clone(),
            card_width: Cell::new(card_width),
            items: RefCell::default(),
            on_open: Rc::default(),
        }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Called with the playlist of the card that was opened.
    pub fn connect_open(&self, f: impl Fn(&PlaylistInfo) + 'static) {
        self.on_open.replace(Some(Box::new(f)));
    }

    pub fn set_card_width(&self, width: i32) {
        if self.card_width.replace(width) != width {
            self.rebuild();
        }
    }

    pub fn set_items(&self, items: Vec<PlaylistInfo>) {
        if *self.items.borrow() == items {
            return;
        }
        self.items.replace(items);
        self.rebuild();
    }

    fn rebuild(&self) {
        while let Some(child) = self.cards.first_child() {
            self.cards.remove(&child);
        }
        let items = self.items.borrow();
        for info in items.iter() {
            self.cards.append(&self.card(info));
        }
        self.root.set_visible(!items.is_empty());
    }

    fn card(&self, info: &PlaylistInfo) -> gtk::Widget {
        let width = self.card_width.get();
        let height = width * 9 / 16;

        // The stack: two sheets peeking out above the poster.
        let sheet = |share: f64, class: &str| {
            gtk::Box::builder()
                .width_request((width as f64 * share) as i32)
                .height_request(5)
                .halign(gtk::Align::Center)
                .css_classes(["k-playlist-sheet", class])
                .build()
        };

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .accessible_role(gtk::AccessibleRole::Presentation)
            .css_classes(["k-thumb"])
            .build();
        let frame = gtk::Box::builder()
            .width_request(width)
            .height_request(height)
            .build();
        let poster = gtk::Overlay::builder()
            .child(&frame)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["k-thumb-frame"])
            .build();
        poster.add_overlay(&picture);
        if let Some(count) = info.badge() {
            let badge = gtk::Box::builder()
                .spacing(4)
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .margin_end(8)
                .margin_bottom(8)
                .css_classes(["k-badge"])
                .build();
            badge.append(
                &gtk::Image::builder()
                    .icon_name("view-list-symbolic")
                    .pixel_size(12)
                    .build(),
            );
            badge.append(
                &gtk::Label::builder()
                    .label(count)
                    .css_classes(["numeric"])
                    .build(),
            );
            poster.add_overlay(&badge);
        }
        let play = gtk::Image::builder()
            .icon_name("media-playback-start-symbolic")
            .pixel_size(20)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["k-playlist-play"])
            .build();
        poster.add_overlay(&play);

        let kicker = gtk::Label::builder()
            .label(info.kicker())
            .xalign(0.0)
            .css_classes(["k-kicker"])
            .build();
        let title = gtk::Label::builder()
            .label(&info.title)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["k-card-title"])
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .margin_top(10)
            .build();
        text.append(&kicker);
        text.append(&title);
        if let Some(owner) = &info.owner {
            text.append(
                &gtk::Label::builder()
                    .label(owner)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["k-card-meta"])
                    .build(),
            );
        }

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(width)
            .build();
        column.append(&sheet(0.82, "back"));
        column.append(&sheet(0.92, "front"));
        column.append(&poster);
        column.append(&text);

        let button = gtk::Button::builder()
            .child(&column)
            .valign(gtk::Align::Start)
            .tooltip_text(gettextrs::gettext("Play with the DJ"))
            .css_classes(["flat", "k-playlist-card"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&info.describe())]);
        let (on_open, opened) = (self.on_open.clone(), info.clone());
        button.connect_clicked(move |_| {
            if let Some(f) = on_open.borrow().as_ref() {
                f(&opened);
            }
        });

        if let Some(url) = info.thumbnail.clone() {
            let (client, picture) = (self.client.clone(), picture.downgrade());
            gspawn!(async move {
                let pixbuf = client.picture(url).await;
                if let (Some(picture), Some(pixbuf)) = (picture.upgrade(), pixbuf) {
                    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
                    picture.add_css_class("loaded");
                }
            });
        }
        button.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kind: PlaylistKind, count: Option<u32>) -> PlaylistInfo {
        PlaylistInfo {
            id: "PL1".into(),
            title: "Late night".into(),
            owner: Some("Somebody".into()),
            count,
            kind,
            thumbnail: None,
        }
    }

    #[test]
    fn a_mix_has_no_count_and_a_playlist_shows_its_own() {
        assert_eq!(info(PlaylistKind::Mix, Some(50)).badge(), None);
        assert_eq!(
            info(PlaylistKind::Playlist, Some(50)).badge().as_deref(),
            Some("50")
        );
        assert_eq!(info(PlaylistKind::Album, None).badge(), None);
        assert_eq!(
            info(PlaylistKind::Playlist, Some(12)).describe(),
            "Playlist: Late night, Somebody, 12"
        );
    }
}
