//! TontooUI embedding for TontooMapsKit.
//!
//! [`MapViewContent`] and [`GlobeViewContent`] wrap a [`MapView`] /
//! [`GlobeView`] for view trees that expect an owned content object. Both
//! views already implement `tontooui::elements::View` directly, so the
//! wrappers simply forward the trait — prefer embedding the views
//! themselves when possible.
//!
//! ```rust,no_run
//! use tontooui::elements::{View, VStack};
//! use mapskit::{MapsConfiguration, MapViewContent};
//!
//! let map = MapViewContent::new(&MapsConfiguration::new());
//! map.map_view().set_center(mapskit::Coordinate::new(52.52, 13.405), 12.0);
//! let stack = VStack::new().child(map);
//! ```

use std::any::Any;

use tontooui::elements::layout::View as TontooView;
use tontooui::renderer::images::ImageLoader;
use tontooui::renderer::text::FontSystem;
use vello::Scene;

use crate::config::MapsConfiguration;
use crate::globe_view::GlobeView;
use crate::map_view::MapView;

/// A content wrapper around a [`MapView`].
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

impl TontooView for MapViewContent {
    fn measure(&mut self, fonts: &mut FontSystem) -> (f32, f32) {
        self.inner.measure(fonts)
    }

    fn place(&mut self, fonts: &mut FontSystem, x: f32, y: f32, width: f32, height: f32) {
        self.inner.place(fonts, x, y, width, height);
    }

    fn draw(&mut self, scene: &mut Scene, fonts: &mut FontSystem, images: &mut ImageLoader<'_>) {
        self.inner.draw(scene, fonts, images);
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.inner.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.inner.mouse_up(x, y);
    }

    fn set_hover(&mut self, x: f32, y: f32) {
        self.inner.set_hover(x, y);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        self.inner.mouse_wheel(dx, dy);
    }

    fn flex(&self) -> f32 {
        1.0
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A content wrapper around a [`GlobeView`].
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

impl TontooView for GlobeViewContent {
    fn measure(&mut self, fonts: &mut FontSystem) -> (f32, f32) {
        self.inner.measure(fonts)
    }

    fn place(&mut self, fonts: &mut FontSystem, x: f32, y: f32, width: f32, height: f32) {
        self.inner.place(fonts, x, y, width, height);
    }

    fn draw(&mut self, scene: &mut Scene, fonts: &mut FontSystem, images: &mut ImageLoader<'_>) {
        self.inner.draw(scene, fonts, images);
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.inner.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.inner.mouse_up(x, y);
    }

    fn set_hover(&mut self, x: f32, y: f32) {
        self.inner.set_hover(x, y);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        self.inner.mouse_wheel(dx, dy);
    }

    fn flex(&self) -> f32 {
        1.0
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
