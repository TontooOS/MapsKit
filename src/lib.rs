//! # TontooMapsKit
//!
//! Maps framework for TontooOS. Follows Apple's MapKit design philosophy:
//! an interactive [`MapView`], place search and geocoding, turn-by-turn
//! routing, overlays, a [`MapCamera`] viewport object and an optional 3D
//! [`GlobeView`].
//!
//! Two map providers are built in with automatic fallback, so apps keep
//! working when one service is down. No API keys are required:
//!
//! | Priority | Provider | Tiles | Search / Geocoding | Places | Routing |
//! |---|---|---|---|---|---|
//! | Primary | OpenStreetMap | tile.openstreetmap.org | Nominatim | Overpass API | OSRM |
//! | Fallback | Photon + CARTO | basemaps.cartocdn.com | Photon | Photon | FOSSGIS OSRM |
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use uikit::prelude::*;
//! use mapskit::prelude::*;
//!
//! fn main() {
//!     let config = MapsConfiguration::new().style(MapStyle::Light);
//!     let map = MapViewContent::new(&config);
//!     map.map_view().set_center(Coordinate::new(52.52, 13.405), 12.0);
//!     map.map_view().add_annotation(
//!         Annotation::new(Coordinate::new(52.52, 13.405), "Berlin"),
//!     );
//!
//!     let mut app = App::new("Maps", 900, 600);
//!     app.set_root_view(View::new(map).with_frame(0.0, 0.0, 900.0, 600.0));
//!     app.run();
//! }
//! ```
//!
//! ## Services without UI
//!
//! ```rust,no_run
//! use mapskit::{ProviderChain, Coordinate};
//!
//! let chain = ProviderChain::default_providers();
//! let results = chain.search("Brandenburger Tor", None, 5)
//!     .expect("search failed");
//! for place in results {
//!     println!("{} at {}", place.name, place.coordinate);
//! }
//! ```

pub mod camera;
pub mod config;
pub mod error;
pub mod ffi;
pub mod globe_view;
pub mod lang;
pub mod map_view;
pub mod overlays;
pub mod providers;
pub mod tiles;
pub mod types;
pub mod uikit_view;

/// Monotonic id counter for annotations and overlays.
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Version of the TontooMapsKit framework (major, minor, patch).
pub const MAPSKIT_VERSION: (u32, u32, u32) = (26, 1, 0);

/// Version string used in HTTP user agents.
pub(crate) const MAPSKIT_VERSION_STR: &str = "26.1";

pub use camera::MapCamera;
pub use config::{MapStyle, MapsConfiguration};
pub use error::MapsError;
pub use globe_view::{GlobeMarker, GlobeView};
pub use lang as localization;
pub use map_view::MapView;
pub use overlays::{Annotation, CircleOverlay, Overlay, Polygon, Polyline};
pub use providers::{MapProvider, ProviderChain};
pub use tiles::{TileCache, TileKey};
pub use types::{
    format_distance, format_duration, Address, BoundingBox, Coordinate, CoordinateSpan, Place,
    PlaceCategory, PlaceInfo, Route, RouteStep, TravelMode,
};
pub use uikit_view::{GlobeViewContent, MapViewContent};

/// Convenience re-exports for a single `use mapskit::prelude::*;`.
pub mod prelude {
    pub use crate::{
        format_distance, format_duration, Address, Annotation, BoundingBox, CircleOverlay,
        Coordinate, CoordinateSpan, GlobeMarker, GlobeView, GlobeViewContent, MapCamera, MapStyle,
        MapsConfiguration, MapsError, MapView, MapViewContent, Overlay, Place, PlaceCategory,
        PlaceInfo, Polygon, Polyline, ProviderChain, Route, RouteStep, TileKey, TravelMode,
    };
}
