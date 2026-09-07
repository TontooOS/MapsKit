//! UIKit embedding for TontooMapsKit.
//!
//! [`MapViewContent`] and [`GlobeViewContent`] implement
//! `uikit::view::ViewContent`, so maps drop into any UIKit view tree exactly
//! like a label or a button.

use crate::config::MapsConfiguration;
use crate::globe_view::GlobeView;
use crate::map_view::MapView;
use gtk::prelude::*;
use uikit::style::Rect;
use uikit::view::ViewContent;

/// A UIKit-compatible wrapper around a [`MapView`].
///
/// ```rust,no_run
/// use uikit::prelude::*;
/// use mapskit::{MapsConfiguration, MapViewContent};
///
/// let map = MapViewContent::new(&MapsConfiguration::new());
/// let view = View::new(map).with_frame(0.0, 0.0, 800.0, 600.0);
/// ```
pub struct MapViewContent {
    inner: MapView,
}

impl MapViewContent {
    /// Creates the content wrapper from a configuration.
    pub fn new(config: &MapsConfiguration) -> Self {
        Self {
            inner: MapView::new(config),
        }
    }

    /// The wrapped [`MapView`].
    pub fn map_view(&self) -> &MapView {
        &self.inner
    }
}

impl ViewContent for MapViewContent {
    fn render(&self, _frame: Rect) -> gtk::Widget {
        self.inner.widget().clone().upcast()
    }
}

/// A UIKit-compatible wrapper around a [`GlobeView`].
///
/// ```rust,no_run
/// use uikit::prelude::*;
/// use mapskit::{Coordinate, GlobeViewContent, MapsConfiguration};
///
/// let globe = GlobeViewContent::new(&MapsConfiguration::new());
/// globe.globe_view().set_center(Coordinate::new(48.137, 11.575));
/// let view = View::new(globe).with_frame(0.0, 0.0, 800.0, 600.0);
/// ```
pub struct GlobeViewContent {
    inner: GlobeView,
}

impl GlobeViewContent {
    /// Creates the content wrapper from a configuration.
    pub fn new(config: &MapsConfiguration) -> Self {
        Self {
            inner: GlobeView::new(config),
        }
    }

    /// The wrapped [`GlobeView`].
    pub fn globe_view(&self) -> &GlobeView {
        &self.inner
    }
}

impl ViewContent for GlobeViewContent {
    fn render(&self, _frame: Rect) -> gtk::Widget {
        self.inner.widget().clone().upcast()
    }
}
