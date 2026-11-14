use std::collections::HashMap;

use chrono::Datelike;

static SPLASHES: std::sync::LazyLock<[Splash; 44]> = std::sync::LazyLock::new(|| {
    [
        // Some random Pipeline facts.
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline was once called Tubefeeder.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline was built for mobile linux devices like the PinePhone. That it works on desktops is just a happy side effect.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "The first commit of Pipeline was {} days ago.",
            )
            .replace(
                "{}",
                &(chrono::Local::now().date_naive() - FIRST_COMMIT)
                    .num_days()
                    .to_string(),
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You are running commit number {} of Pipeline.",
            )
            .replace("{}", &crate::config::NUMBER_OF_COMMITS.to_string())
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You are running release number {} of Pipeline.",
            )
            .replace("{}", &crate::config::NUMBER_OF_TAGS.to_string())
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline is built from {} lines of Rust code.",
            )
            .replace("{}", &crate::config::LINES_OF_RUST.to_string())
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline is built from {} lines of Blueprint UI markup.",
            )
            .replace("{}", &crate::config::LINES_OF_BLUEPRINT.to_string())
        }),
        Splash::did_you_know(|| {
            // Translators: "Splash" as in the splash text from Minecraft: https://minecraft.wiki/w/Splash.
            gettextrs::pgettext(
                "Displayed as a splash text",
                "There are {} different splash texts which can be displayed here.",
            )
            .replace("{}", &SPLASHES.len().to_string())
        }),
        Splash::did_you_know(|| {
            // Via the coupon collector's problem.
            let num_splashes = SPLASHES.len();
            let harmonic_to_num: f64 = (1..=num_splashes).map(|i| 1.0 / (i as f64)).sum();
            let num_reloads: f64 = (num_splashes as f64) * harmonic_to_num;
            // Translators: "Splash" as in the splash text from Minecraft: https://minecraft.wiki/w/Splash.
            gettextrs::pgettext("Displayed as a splash text", "In expectation, you will need to reload {} times to have seen all splash texts at least once.")
                .replace("{}", &format!("{num_reloads:.2}"))
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline is translated in {languages} different languages by {translators} translators",
            )
            .replace(
                "{languages}",
                &crate::config::NUMBER_OF_TRANSLATED_LANGUAGES.to_string(),
            )
            .replace(
                "{translators}",
                &crate::config::NUMBER_OF_TRANSLATORS.to_string(),
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You can help translate Pipeline using <a href=\"https://hosted.weblate.org/engage/schmiddi-on-mobile/\">Weblate</a>.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "The source code of Pipeline is hosted on <a href=\"https://gitlab.com/schmiddi-on-mobile/pipeline\">GitLab</a>.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "We have a <a href=\"https://matrix.to/#/%23pipelineapp:matrix.org\">Matrix room</a> where you can come and talk with us.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline has a <a href=\"https://mobile.schmidhuberj.de/pipeline\">website</a>.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You can get Pipeline on <a href=\"https://flathub.org/apps/de.schmidhuberj.tubefeeder\">Flathub</a>.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "We use <a href=\"https://github.com/Rafostar/clapper\">Clapper</a> as the video player.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "We use <a href=\"https://codeberg.org/ThetaDev/rustypipe/\">rustypipe</a> and <a href=\"https://piped.video/\">Piped</a> to get data from YouTube.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "The creator of Pipeline has also built other applications, like <a href=\"https://flathub.org/apps/de.schmidhuberj.DieBahn\">Railway</a> and <a href=\"https://flathub.org/apps/de.schmidhuberj.Flare\">Flare</a>.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "When using Piped to fetch data, Pipeline will be able to fetch your subscriptions faster with more public Piped instances. So consider <a href=\"https://docs.piped.video/docs/self-hosting/\">hosting Piped</a> and making it available for everyone.",
            )
        }),
        Splash::did_you_know(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "No animals were harmed while making this application. But two cats were petted.",
            )
        }),
        // Some features of Pipeline
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Add videos to your watch-later list by bringing up their context menu, or directly when watching videos.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Seeing something you don't like? You can hide videos from your feed based on the title and uploader of the video.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You can hide videos you already watched. Enable this by managing your filters.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You can quickly toggle on and off filters in the feed from the filter dialog.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Want to keep your subscriptions from YouTube or NewPipe? You can import those into Pipeline.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Want to bring your own video player instead of the one built-in? You can manage an external video player in the settings and tell Pipeline to use it by default.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "You want to keep a video you watched? You can directly download it from the video page.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Want to share a video? You can copy its URL to the clipboard directly from the video page.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Are you hosting your own Piped instance? You can tell Pipeline to use it in the settings.",
            )
        }),
        Splash::feature(|| {
            // Translators: "Splash" as in the splash text from Minecraft: https://minecraft.wiki/w/Splash.
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Not interested in those splash texts? You can disable them in the settings.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Don't like YouTube anymore? Pipeline also works with <a href=\"https://joinpeertube.org\">PeerTube</a>.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Searching for something? Pipeline can help you with that and search for channels and videos on YouTube and Peertube.",
            )
        }),
        Splash::feature(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pipeline has four different ways to fetch videos from YouTube. Just to be sure you don't miss any of your precious cat videos.",
            )
        }),
        // Other YouTube applications (and other video sharing platforms).
        Splash::also_try(|| "<a href=\"https://newpipe.net/\">NewPipe</a>".into()),
        Splash::also_try(|| "<a href=\"https://invidious.io/\">Invidious</a>".into()),
        Splash::also_try(|| "<a href=\"https://piped.video/\">Piped</a>".into()),
        Splash::also_try(|| "<a href=\"https://freetubeapp.io/\">FreeTube</a>".into()),
        Splash::also_try(|| "<a href=\"https://github.com/yattee/yattee\">Yattee</a>".into()),
        Splash::also_try(|| "<a href=\"https://libretube.dev/\">LibreTube</a>".into()),
        Splash::also_try(|| "<a href=\"https://joinpeertube.org\">PeerTube</a>".into()),
        // Miscellaneous
        Splash::unspecified(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Have ideas for further splash text? Tell us on <a href=\"https://matrix.to/#/%23pipelineapp:matrix.org\">Matrix</a>.",
            )
        }),
        Splash::unspecified(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Pretty sure {year} will be the year of the Linux desktop.",
            )
            .replace("{year}", &(chrono::Local::now().year() + 1).to_string())
        }),
        Splash::unspecified(|| {
            // Translators: A reference to the "I use Arch btw" meme.
            gettextrs::pgettext("Displayed as a splash text", "I can be used on Arch btw.")
        }),
        Splash::unspecified(|| {
            gettextrs::pgettext(
                "Displayed as a splash text",
                "Do you want to advertise here? You can't.",
            )
        }),
    ]
});

const FIRST_COMMIT: chrono::NaiveDate = chrono::NaiveDate::from_ymd_opt(2021, 3, 1).unwrap();

// Uses the year 2000 for all dates.
static DATE_SPLASHES: std::sync::LazyLock<HashMap<chrono::NaiveDate, Splash>> =
    std::sync::LazyLock::new(|| {
        HashMap::from([(
            FIRST_COMMIT.with_year(2000).unwrap(),
            Splash::unspecified(|| {
                // Translators: Please keep the confetti emojis.
                gettextrs::pgettext(
                    "Displayed as a splash text",
                    "🎉 Pipeline turned {} years old today. Happy Birthday! 🎉",
                )
                .replace(
                    "{}",
                    &(chrono::Local::now().year() - FIRST_COMMIT.year()).to_string(),
                )
            }),
        )])
    });

#[derive(Clone, Copy)]
pub enum SplashKind {
    DidYouKnow,
    Feature,
    AlsoTry,
    Unspecified,
}

pub struct Splash {
    kind: SplashKind,
    text_constructor: Box<dyn Fn() -> String + 'static + Sync + Send>,
}

impl Splash {
    fn new<F: 'static + Fn() -> String + Sync + Send>(
        kind: SplashKind,
        text_constructor: F,
    ) -> Self {
        Self {
            kind,
            text_constructor: Box::new(text_constructor) as Box<dyn Fn() -> String + Sync + Send>,
        }
    }

    fn did_you_know<F: 'static + Fn() -> String + Sync + Send>(text_constructor: F) -> Self {
        Self::new(SplashKind::DidYouKnow, text_constructor)
    }

    fn feature<F: 'static + Fn() -> String + Sync + Send>(text_constructor: F) -> Self {
        Self::new(SplashKind::Feature, text_constructor)
    }

    fn also_try<F: 'static + Fn() -> String + Sync + Send>(text_constructor: F) -> Self {
        Self::new(SplashKind::AlsoTry, text_constructor)
    }

    fn unspecified<F: 'static + Fn() -> String + Sync + Send>(text_constructor: F) -> Self {
        Self::new(SplashKind::Unspecified, text_constructor)
    }

    pub fn label(&self) -> String {
        match self.kind {
            // Translators: Used to construct splash texts for different applications and platforms.
            SplashKind::AlsoTry => {
                gettextrs::pgettext("Displayed as a splash text", "Also try {}!")
                    .replace("{}", &(self.text_constructor)())
            }
            _ => (self.text_constructor)(),
        }
    }

    pub fn random() -> &'static Splash {
        use rand::prelude::*;

        let date = chrono::Local::now().date_naive().with_year(2000).unwrap();
        if let Some(splash) = DATE_SPLASHES.get(&date) {
            splash
        } else {
            // There always exists at least one splash, therefore it is save to unwrap.
            SPLASHES.choose(&mut rand::rng()).unwrap()
        }
    }
}
