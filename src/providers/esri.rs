//! Tile provider: Esri ArcGIS services (no API key).
//!
//! Serves raster tiles from the public ArcGIS services
//! (`server.arcgisonline.com`): World Imagery satellite tiles for
//! `MapStyle::Satellite` and the World Street Map (light, modern
//! Google-Maps-like cartography) for `MapStyle::Light`. This provider is
//! tile-only; search, geocoding and routing are not supported and always
//! return a `MapsError::Provider` error, so the `ProviderChain` falls
//! through to the regular providers for those services.

use super::MapProvider;
use crate::config::{MapStyle, MapsConfiguration};
use crate::error::MapsError;
use crate::types::{Address, Coordinate, Place, PlaceCategory, Route, TravelMode};

/// Esri backed tile provider (satellite + light street map).
pub struct EsriProvider {
    style: MapStyle,
}

impl EsriProvider {
    pub fn new() -> Self {
        Self {
            style: MapStyle::Satellite,
        }
    }

    pub fn with_config(config: &MapsConfiguration) -> Self {
        Self {
            style: config.style,
        }
    }
}

impl Default for EsriProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MapProvider for EsriProvider {
    fn name(&self) -> &'static str {
        "Esri"
    }

    /// Serves satellite imagery and the light street map.
    fn supports_style(&self, style: MapStyle) -> bool {
        matches!(style, MapStyle::Satellite | MapStyle::Light)
    }

    fn search(
        &self,
        _query: &str,
        _near: Option<Coordinate>,
        _limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        Err(MapsError::Provider("satellite tiles only".into()))
    }

    fn reverse_geocode(&self, _coordinate: Coordinate) -> Result<Address, MapsError> {
        Err(MapsError::Provider("satellite tiles only".into()))
    }

    fn nearby(
        &self,
        _center: Coordinate,
        _radius_m: u32,
        _categories: &[PlaceCategory],
        _limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        Err(MapsError::Provider("satellite tiles only".into()))
    }

    fn route(
        &self,
        _from: Coordinate,
        _to: Coordinate,
        _mode: TravelMode,
    ) -> Result<Route, MapsError> {
        Err(MapsError::Provider("satellite tiles only".into()))
    }

    /// ArcGIS serves row-first: `/tile/{z}/{y}/{x}`. Satellite uses World
    /// Imagery; the light style uses the World Street Map.
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
        match self.style {
            MapStyle::Light => format!(
                "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/tile/{z}/{y}/{x}"
            ),
            _ => format!(
                "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_satellite_and_light() {
        let provider = EsriProvider::new();
        assert!(provider.supports_style(MapStyle::Satellite));
        let light =
            EsriProvider::with_config(&crate::config::MapsConfiguration::new().style(MapStyle::Light));
        assert!(light.supports_style(MapStyle::Light));
        assert!(!provider.supports_style(MapStyle::Dark));
        assert!(!provider.supports_style(MapStyle::Standard));
    }

    #[test]
    fn tile_urls_follow_arcgis_order() {
        let provider = EsriProvider::new();
        let url = provider.tile_url(5, 7, 3);
        assert!(url.contains("/World_Imagery/MapServer/tile/3/7/5"));

        let light =
            EsriProvider::with_config(&crate::config::MapsConfiguration::new().style(MapStyle::Light));
        let url = light.tile_url(5, 7, 3);
        assert!(url.contains("/World_Street_Map/MapServer/tile/3/7/5"));
    }

    #[test]
    fn services_are_unsupported() {
        let provider = EsriProvider::new();
        assert!(provider.search("x", None, 1).is_err());
        assert!(provider.reverse_geocode(Coordinate::ZERO).is_err());
        assert!(provider.route(Coordinate::ZERO, Coordinate::ZERO, TravelMode::Driving).is_err());
    }
}
