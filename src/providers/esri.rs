//! Satellite imagery provider: Esri World Imagery.
//!
//! Serves raster satellite tiles from the public ArcGIS World Imagery
//! service (`server.arcgisonline.com`). This provider is tile-only; search,
//! geocoding and routing are not supported and always return
//! a `MapsError::Provider` error, so the `ProviderChain` falls through to the
//! regular providers for those services.

use super::MapProvider;
use crate::config::MapStyle;
use crate::error::MapsError;
use crate::types::{Address, Coordinate, Place, PlaceCategory, Route, TravelMode};

/// Esri World Imagery backed satellite tile provider.
pub struct EsriProvider;

impl EsriProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn with_config(_config: &crate::config::MapsConfiguration) -> Self {
        Self::new()
    }
}

impl Default for EsriProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MapProvider for EsriProvider {
    fn name(&self) -> &'static str {
        "Esri World Imagery"
    }

    /// Only serves satellite imagery.
    fn supports_style(&self, style: MapStyle) -> bool {
        matches!(style, MapStyle::Satellite)
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

    /// ArcGIS serves row-first: `/tile/{z}/{y}/{x}`.
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
        format!(
            "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_satellite() {
        let provider = EsriProvider::new();
        assert!(provider.supports_style(MapStyle::Satellite));
        assert!(!provider.supports_style(MapStyle::Light));
        assert!(!provider.supports_style(MapStyle::Dark));
        assert!(!provider.supports_style(MapStyle::Standard));
    }

    #[test]
    fn tile_urls_follow_arcgis_order() {
        let provider = EsriProvider::new();
        let url = provider.tile_url(5, 7, 3);
        assert!(url.contains("/World_Imagery/MapServer/tile/3/7/5"));
    }

    #[test]
    fn services_are_unsupported() {
        let provider = EsriProvider::new();
        assert!(provider.search("x", None, 1).is_err());
        assert!(provider.reverse_geocode(Coordinate::ZERO).is_err());
        assert!(provider.route(Coordinate::ZERO, Coordinate::ZERO, TravelMode::Driving).is_err());
    }
}
