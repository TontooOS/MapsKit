//! `MapsConfiguration` — the settings object passed to every view and
//! service, following Apple's MapKit configuration style.

use crate::types::PlaceCategory;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Visual style of the base map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MapStyle {
    /// Light basemap: classic OSM cartography from the Fastly CDN (the
    /// same tiles Leaflet shows by default).
    Light,
    /// Dark basemap (Esri dark gray canvas on the fallback provider).
    Dark,
    /// Classic OpenStreetMap cartography, same imagery as light.
    Standard,
    /// Satellite imagery (Esri World Imagery on the satellite provider).
    Satellite,
}

impl Default for MapStyle {
    fn default() -> Self {
        Self::Light
    }
}

/// Configuration for MapsKit views and services.
#[derive(Debug, Clone)]
pub struct MapsConfiguration {
    /// Base map style.
    pub style: MapStyle,
    /// Whether points of interest are rendered on the map.
    pub shows_points_of_interest: bool,
    /// When non-empty, only these categories are shown.
    pub point_of_interest_filter: Vec<PlaceCategory>,
    /// Whether the user location is shown (requires CoreLocation in the app).
    pub shows_user_location: bool,
    /// Custom tile cache directory. Defaults to
    /// `$XDG_CACHE_HOME/tontoos/mapskit` or `~/.cache/tontoos/mapskit`.
    pub cache_directory: Option<PathBuf>,
    /// User agent sent with every provider request.
    pub user_agent: String,
    /// Tile request timeout in seconds.
    pub timeout_seconds: u64,
    /// Whether the 3D globe may fetch an earth texture from the providers.
    pub globe_earth_texture: bool,
}

impl Default for MapsConfiguration {
    fn default() -> Self {
        Self {
            style: MapStyle::default(),
            shows_points_of_interest: true,
            point_of_interest_filter: Vec::new(),
            shows_user_location: false,
            cache_directory: None,
            user_agent: format!("TontooOS-MapsKit/{}", crate::MAPSKIT_VERSION_STR),
            timeout_seconds: 10,
            globe_earth_texture: true,
        }
    }
}

impl MapsConfiguration {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the base map style.
    pub fn style(mut self, style: MapStyle) -> Self {
        self.style = style;
        self
    }

    /// Enables or disables points of interest on the map.
    pub fn shows_points_of_interest(mut self, show: bool) -> Self {
        self.shows_points_of_interest = show;
        self
    }

    /// Restricts the displayed points of interest to the given categories.
    pub fn point_of_interest_filter(mut self, categories: Vec<PlaceCategory>) -> Self {
        self.point_of_interest_filter = categories;
        self
    }

    /// Overrides the tile cache directory.
    pub fn cache_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_directory = Some(path.into());
        self
    }

    /// Overrides the HTTP user agent.
    pub fn user_agent(mut self, agent: impl Into<String>) -> Self {
        self.user_agent = agent.into();
        self
    }

    /// Resolved cache directory, creating it when missing.
    pub fn resolved_cache_dir(&self) -> PathBuf {
        if let Some(dir) = &self.cache_directory {
            return dir.clone();
        }
        let base = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".cache")
            });
        base.join("tontoos").join("mapskit")
    }

    /// Whether a category passes the POI filter.
    pub fn category_allowed(&self, category: PlaceCategory) -> bool {
        self.point_of_interest_filter.is_empty()
            || self.point_of_interest_filter.contains(&category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let config = MapsConfiguration::new();
        assert_eq!(config.style, MapStyle::Light);
        assert!(config.shows_points_of_interest);
        assert!(config.timeout_seconds >= 5);
        assert!(config.user_agent.starts_with("TontooOS-MapsKit/"));
    }

    #[test]
    fn builder() {
        let config = MapsConfiguration::new()
            .style(MapStyle::Dark)
            .shows_points_of_interest(false)
            .cache_directory("/tmp/tiles");
        assert_eq!(config.style, MapStyle::Dark);
        assert!(!config.shows_points_of_interest);
        assert_eq!(config.resolved_cache_dir(), PathBuf::from("/tmp/tiles"));
    }

    #[test]
    fn poi_filter() {
        let config =
            MapsConfiguration::new().point_of_interest_filter(vec![PlaceCategory::Cafe]);
        assert!(config.category_allowed(PlaceCategory::Cafe));
        assert!(!config.category_allowed(PlaceCategory::Fuel));

        let open = MapsConfiguration::new();
        assert!(open.category_allowed(PlaceCategory::Fuel));
    }
}
