use gdk::gio;
use gdk::prelude::{Cast, ListModelExt};
use gdk::subclass::prelude::ObjectSubclassIsExt;
use glib::Object;
use gtk::FilterChange;
use gtk::prelude::FilterExt;

use super::Filter;

gtk::glib::wrapper! {
    pub struct FilterStore(ObjectSubclass<imp::FilterStore>)
        @extends gtk::Filter,
        @implements gio::ListModel;
}

impl Default for FilterStore {
    fn default() -> Self {
        Object::builder().build()
    }
}

impl FilterStore {
    /// Toggle a filter in the list.
    ///
    /// Returns `true` if the filter was added to the list, and `false` if it was removed.
    pub fn toggle(&self, filter: Filter) -> bool {
        let mut filters = self.imp().filters.borrow_mut();

        let index = filters.iter().position(|e| e == &filter);
        match index {
            Some(index) => {
                filters.remove(index);
                drop(filters);
                self.items_changed(index as u32, 1, 0);
                self.changed(FilterChange::LessStrict);
                false
            }
            None => {
                let prev_len = filters.len();
                filters.push(filter);
                drop(filters);
                self.items_changed(prev_len as u32, 0, 1);
                self.changed(FilterChange::MoreStrict);
                true
            }
        }
    }

    pub fn has_filter(&self, filter: &Filter) -> bool {
        let filters = self.imp().filters.borrow_mut();
        filters.iter().any(|e| e == filter)
    }

    fn items_changed(&self, position: u32, removed: u32, added: u32) {
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, removed, added);
    }
}

mod imp {
    use chrono::{Local, TimeDelta};
    use gdk::gio;
    use glib::Properties;
    use glib::object::Cast;
    use glib::subclass::types::ObjectSubclassExt;
    use glib::types::StaticType;
    use gtk::prelude::{FilterExt, ObjectExt};
    use gtk::subclass::filter::{FilterImpl, FilterImplExt};
    use gtk::{FilterChange, glib};
    use std::cell::{Cell, RefCell};

    use gdk::subclass::prelude::ListModelImpl;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};

    use crate::backend::{Filter, Video};

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::FilterStore)]
    pub struct FilterStore {
        pub(super) filters: RefCell<Vec<Filter>>,

        #[property(get, set = Self::set_hide_shorts)]
        hide_shorts: Cell<bool>,
        #[property(get, set = Self::set_hide_today)]
        hide_today: Cell<bool>,
        #[property(get, set = Self::set_hide_requires_payment)]
        hide_requires_payment: Cell<bool>,
        #[property(get, set = Self::set_hide_watched)]
        hide_watched: Cell<bool>,

        #[property(get, set = Self::set_enabled)]
        enabled: Cell<bool>,
    }

    impl FilterStore {
        fn set_hide_shorts(&self, value: bool) {
            self.hide_shorts.set(value);
            self.obj().changed(if value {
                FilterChange::MoreStrict
            } else {
                FilterChange::LessStrict
            });
        }

        fn set_hide_today(&self, value: bool) {
            self.hide_today.set(value);
            self.obj().changed(if value {
                FilterChange::MoreStrict
            } else {
                FilterChange::LessStrict
            });
        }

        fn set_hide_requires_payment(&self, value: bool) {
            self.hide_requires_payment.set(value);
            self.obj().changed(if value {
                FilterChange::MoreStrict
            } else {
                FilterChange::LessStrict
            });
        }

        fn set_hide_watched(&self, value: bool) {
            self.hide_watched.set(value);
            self.obj().changed(if value {
                FilterChange::MoreStrict
            } else {
                FilterChange::LessStrict
            });
        }

        fn set_enabled(&self, value: bool) {
            self.enabled.set(value);
            self.obj().changed(if value {
                FilterChange::MoreStrict
            } else {
                FilterChange::LessStrict
            });
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FilterStore {
        const NAME: &'static str = "TFFilterStore";
        type Type = super::FilterStore;
        type Interfaces = (gio::ListModel,);
        type ParentType = gtk::Filter;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FilterStore {}

    impl ListModelImpl for FilterStore {
        fn item_type(&self) -> glib::Type {
            Filter::static_type()
        }

        fn n_items(&self) -> u32 {
            self.filters.borrow().len().try_into().unwrap_or_default()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.filters.borrow();

            list.get(position as usize)
                .map(|o| o.clone().upcast::<glib::Object>())
        }
    }

    impl FilterImpl for FilterStore {
        fn strictness(&self) -> gtk::FilterMatch {
            self.parent_strictness()
        }

        fn match_(&self, item: &glib::Object) -> bool {
            let obj = self.obj();
            if !obj.enabled() {
                return true;
            }

            let Some(video) = item.dynamic_cast_ref::<Video>() else {
                return false;
            };

            let matches_short =
                obj.hide_shorts() && video.duration().is_some_and(|d| d < TimeDelta::minutes(1));
            let matches_today = obj.hide_today()
                && video.uploaded().with_timezone(&Local).date_naive() == Local::now().date_naive();
            let matches_requires_payment = obj.hide_requires_payment() && video.requires_payment();
            let matches_watched = obj.hide_watched() && video.watched();

            !matches_short
                && !matches_today
                && !matches_requires_payment
                && !matches_watched
                && self.filters.borrow().iter().all(|f| f.match_(item))
        }
    }
}
