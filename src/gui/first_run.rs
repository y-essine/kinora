//! Kinora: "How do you want to watch?" (see `docs/design.md`, "First run").
//!
//! Shown once on the first launch, skippable, and again from the preferences.
//! A way to watch is only a preset of settings that already exist (the
//! extensions that are on, and *reduce effects*): nothing in the app asks
//! which one was picked, and every part of it can be changed afterwards.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::gui::window::Window;

const DONE: &str = "first-run-done";
const DISABLED: &str = "disabled-extensions";
const REDUCE_EFFECTS: &str = "reduce-effects";

const ACCOUNT: &str = "account";
const HOME_CHIPS: &str = "home-chips";
const DJ: &str = "dj";

/// A way to watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No account, nothing sent to one: subscriptions stay on this computer.
    Private,
    /// As little as possible: no effects, no extras.
    Lite,
    /// The YouTube account: its Home, Up next and history.
    Connected,
    /// Connected, plus the extras (the DJ).
    Fun,
}

/// What a mode sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Preset {
    /// The account extension; `None` leaves it as it is.
    account: Option<bool>,
    home_chips: bool,
    dj: bool,
    reduce_effects: bool,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Private, Mode::Lite, Mode::Connected, Mode::Fun];

    fn preset(self) -> Preset {
        match self {
            Mode::Private => Preset {
                account: Some(false),
                home_chips: true,
                dj: false,
                reduce_effects: false,
            },
            Mode::Lite => Preset {
                account: None,
                home_chips: false,
                dj: false,
                reduce_effects: true,
            },
            Mode::Connected => Preset {
                account: Some(true),
                home_chips: true,
                dj: false,
                reduce_effects: false,
            },
            Mode::Fun => Preset {
                account: Some(true),
                home_chips: true,
                dj: true,
                reduce_effects: false,
            },
        }
    }

    /// Whether the mode is about the account (so offer to connect one).
    fn wants_account(self) -> bool {
        self.preset().account == Some(true)
    }

    fn title(self) -> String {
        match self {
            Mode::Private => gettextrs::gettext("Private"),
            Mode::Lite => gettextrs::gettext("Lite"),
            Mode::Connected => gettextrs::gettext("Connected"),
            Mode::Fun => gettextrs::gettext("Fun"),
        }
    }

    fn description(self) -> String {
        match self {
            Mode::Private => gettextrs::gettext(
                "No account. Your subscriptions and history stay on this computer.",
            ),
            Mode::Lite => gettextrs::gettext("Quick and quiet: no effects, no extras, less data."),
            Mode::Connected => gettextrs::gettext(
                "Your YouTube account: your Home, your recommendations, your history.",
            ),
            Mode::Fun => {
                gettextrs::gettext("Connected, with the extras: a DJ that never stops the music.")
            }
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Mode::Private => "channel-secure-symbolic",
            Mode::Lite => "power-profile-power-saver-symbolic",
            Mode::Connected => "avatar-default-symbolic",
            Mode::Fun => "audio-headphones-symbolic",
        }
    }

    /// The disabled-extensions list with this mode applied to `disabled`
    /// (ids the mode does not know stay as they are), and *reduce effects*.
    fn apply_to(self, disabled: &[String]) -> (Vec<String>, bool) {
        let preset = self.preset();
        let mut list: Vec<String> = disabled.to_vec();
        let mut set = |id: &str, on: bool| {
            list.retain(|d| d != id);
            if !on {
                list.push(id.to_owned());
            }
        };
        if let Some(account) = preset.account {
            set(ACCOUNT, account);
        }
        set(HOME_CHIPS, preset.home_chips);
        set(DJ, preset.dj);
        (list, preset.reduce_effects)
    }

    /// The mode the settings are in now, if they match one exactly.
    fn current(disabled: &[String], reduce_effects: bool, connected: bool) -> Option<Mode> {
        let on = |id: &str| !disabled.iter().any(|d| d == id);
        Mode::ALL.into_iter().find(|mode| {
            let p = mode.preset();
            p.reduce_effects == reduce_effects
                && p.home_chips == on(HOME_CHIPS)
                && p.dj == on(DJ)
                && match p.account {
                    Some(true) => on(ACCOUNT) && connected,
                    Some(false) => !on(ACCOUNT) || !connected,
                    None => true,
                }
        })
    }
}

fn disabled(settings: &gio::Settings) -> Vec<String> {
    settings
        .strv(DISABLED)
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Whether the screen should come up by itself: not seen yet.
pub fn is_due(settings: &gio::Settings) -> bool {
    !settings.boolean(DONE)
}

/// Show the screen over `window`.
pub fn present(window: &Window, settings: &gio::Settings) {
    let dialog = adw::Dialog::builder()
        .content_width(640)
        .css_classes(["k-first-run"])
        .build();
    let chosen: Rc<Cell<Option<Mode>>> = Rc::default();
    let connected = window.client().has_account();
    let now = Mode::current(
        &disabled(settings),
        settings.boolean(REDUCE_EFFECTS),
        connected,
    );

    let title = gtk::Label::builder()
        .label(gettextrs::gettext("How do you want to watch?"))
        .css_classes(["k-display", "k-first-run-title"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let subtitle = gtk::Label::builder()
        .label(gettextrs::gettext(
            "Pick a start. Everything can be changed later in the preferences.",
        ))
        .css_classes(["k-card-meta"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();

    let go = gtk::Button::builder()
        .label(gettextrs::gettext("Continue"))
        .css_classes(["pill", "suggested-action"])
        .sensitive(now.is_some())
        .build();
    let skip = gtk::Button::builder()
        .label(gettextrs::gettext("Skip"))
        .css_classes(["pill", "flat"])
        .build();

    let cards = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(2)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    let mut group: Option<gtk::ToggleButton> = None;
    for mode in Mode::ALL {
        let icon = gtk::Image::builder()
            .icon_name(mode.icon())
            .pixel_size(20)
            .valign(gtk::Align::Start)
            .css_classes(["k-first-run-icon"])
            .build();
        let name = gtk::Label::builder()
            .label(mode.title())
            .xalign(0.0)
            .css_classes(["k-card-title"])
            .build();
        let description = gtk::Label::builder()
            .label(mode.description())
            .xalign(0.0)
            .wrap(true)
            .css_classes(["k-card-meta"])
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();
        text.append(&name);
        text.append(&description);
        let content = gtk::Box::builder().spacing(14).build();
        content.append(&icon);
        content.append(&text);
        let card = gtk::ToggleButton::builder()
            .child(&content)
            .css_classes(["k-mode-card"])
            .active(now == Some(mode))
            .build();
        card.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}. {}",
            mode.title(),
            mode.description()
        ))]);
        match &group {
            Some(first) => card.set_group(Some(first)),
            None => group = Some(card.clone()),
        }
        card.connect_toggled(glib::clone!(
            #[strong]
            chosen,
            #[weak]
            go,
            move |card| {
                if card.is_active() {
                    chosen.set(Some(mode));
                    go.set_sensitive(true);
                }
            }
        ));
        let child = gtk::FlowBoxChild::builder()
            .child(&card)
            .focusable(false)
            .build();
        cards.append(&child);
    }
    chosen.set(now);

    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    buttons.append(&skip);
    buttons.append(&go);

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .css_classes(["k-first-run-body"])
        .build();
    column.append(&title);
    column.append(&subtitle);
    column.append(&cards);
    column.append(&buttons);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&column)
        .build();
    dialog.set_child(Some(&scroller));
    dialog.set_default_widget(Some(&go));

    skip.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    go.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        window,
        #[strong]
        settings,
        move |_| {
            dialog.close();
            let Some(mode) = chosen.get() else { return };
            log::info!("First run: {mode:?}");
            let (list, reduce_effects) = mode.apply_to(&disabled(&settings));
            if let Err(e) = settings
                .set_strv(DISABLED, list)
                .and_then(|_| settings.set_boolean(REDUCE_EFFECTS, reduce_effects))
            {
                log::warn!("Could not store the way to watch: {e}");
            }
            if mode.wants_account() && !window.client().has_account() {
                let _ =
                    gtk::prelude::WidgetExt::activate_action(&window, "win.connect-account", None);
            }
        }
    ));
    // Seen, however it was left (Skip, Esc or Continue).
    dialog.connect_closed(glib::clone!(
        #[strong]
        settings,
        move |_| {
            let _ = settings.set_boolean(DONE, true);
        }
    ));
    dialog.present(Some(window));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn private_switches_the_account_off_and_keeps_other_extensions() {
        let (disabled, reduce) = Mode::Private.apply_to(&list(&["somebody-elses"]));
        assert_eq!(disabled, list(&["somebody-elses", "account", "dj"]));
        assert!(!reduce);
    }

    #[test]
    fn lite_reduces_effects_and_leaves_the_account_alone() {
        let (disabled, reduce) = Mode::Lite.apply_to(&list(&["account"]));
        assert_eq!(disabled, list(&["account", "home-chips", "dj"]));
        assert!(reduce);
        let (disabled, _) = Mode::Lite.apply_to(&[]);
        assert!(!disabled.contains(&"account".to_owned()));
    }

    #[test]
    fn fun_switches_everything_on() {
        let (disabled, reduce) = Mode::Fun.apply_to(&list(&["account", "dj", "home-chips"]));
        assert!(disabled.is_empty());
        assert!(!reduce);
        let (disabled, _) = Mode::Connected.apply_to(&[]);
        assert_eq!(disabled, list(&["dj"]));
    }

    #[test]
    fn the_current_mode_is_recognised_after_applying_it() {
        for mode in Mode::ALL {
            let (disabled, reduce) = mode.apply_to(&[]);
            let connected = mode.wants_account();
            assert_eq!(Mode::current(&disabled, reduce, connected), Some(mode));
        }
        // The defaults with an account are "Fun"; a custom mix is no mode.
        assert_eq!(Mode::current(&[], false, true), Some(Mode::Fun));
        assert_eq!(Mode::current(&list(&["home-chips"]), false, true), None);
    }
}
