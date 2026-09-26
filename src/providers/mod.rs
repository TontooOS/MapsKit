//! Map service providers and the fallback chain.
//!
//! MapsKit ships four providers:
//!
//! 1. [`BackendProvider`] (primary) — the TontooOS backend maps API, which
//!    proxies and caches tiles, search, places and routes. All TontooOS map
//!    clients use it; when it is unreachable the chain falls through to the
//!    direct providers below.
//! 2. [`EsriProvider`] — Esri World Imagery satellite raster tiles
//!    (`MapStyle::Satellite` only).
//! 3. [`OsmProvider`] — OpenStreetMap raster tiles, Nominatim
//!    search/geocoding, Overpass place details and OSRM routing.
//! 4. [`PhotonProvider`] (fallback) — Esri dark canvas tiles, Komoot Photon
//!    search/geocoding and the FOSSGIS OSRM instances for routing.
//!
//! A [`ProviderChain`] tries every provider in order and returns the first
//! successful result, so apps keep working when one service is down.

pub mod backend;
pub mod esri;
pub mod osm;
pub mod photon;

use crate::config::MapsConfiguration;
use crate::error::MapsError;
use crate::types::{Address, Coordinate, Place, Route, TravelMode};
use std::sync::Arc;

/// A map data provider. All methods are blocking; call them from worker
/// threads, not from the UI thread.
pub trait MapProvider: Send + Sync {
    /// Human readable provider name (`"OpenStreetMap"`, `"Photon + OSM/Esri"`).
    fn name(&self) -> &'static str;

    /// Whether the provider endpoints are reachable. Cheap check used before
    /// falling back.
    fn is_available(&self) -> bool {
        true
    }

    /// Whether this provider can serve base map tiles for the given style.
    /// Used by the map views to pick a tile source.
    fn supports_style(&self, _style: crate::config::MapStyle) -> bool {
        true
    }

    /// Free-text search for places and addresses.
    fn search(
        &self,
        query: &str,
        near: Option<Coordinate>,
        limit: usize,
    ) -> Result<Vec<Place>, MapsError>;

    /// Reverse geocode a coordinate into a structured address.
    fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError>;

    /// Enrich a place with details (phone, website, opening hours, ...).
    /// The default returns the place unchanged.
    fn place_details(&self, place: &Place) -> Result<Place, MapsError> {
        Ok(place.clone())
    }

    /// Find places of the given categories around a center within a radius
    /// in meters.
    fn nearby(
        &self,
        center: Coordinate,
        radius_m: u32,
        categories: &[crate::types::PlaceCategory],
        limit: usize,
    ) -> Result<Vec<Place>, MapsError>;

    /// Compute a route between two coordinates.
    fn route(
        &self,
        from: Coordinate,
        to: Coordinate,
        mode: TravelMode,
    ) -> Result<Route, MapsError>;

    /// Tile URL template for slippy map tiles at `z/x/y`.
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String;
}

/// Ordered list of providers with automatic fallback.
///
/// ```rust,no_run
/// use mapskit::providers::ProviderChain;
///
/// let chain = ProviderChain::default_providers();
/// // Tries OpenStreetMap first, Photon/fallback when OSM fails.
/// ```
pub struct ProviderChain {
    providers: Vec<Arc<dyn MapProvider>>,
}

impl ProviderChain {
    /// Creates a chain from explicit providers.
    pub fn new(providers: Vec<Arc<dyn MapProvider>>) -> Self {
        Self { providers }
    }

    /// The default chain: TontooOS backend first, Esri satellite tiles,
    /// OpenStreetMap primary, Photon + OSM/Esri fallback.
    pub fn default_providers() -> Self {
        Self::new(vec![
            Arc::new(backend::BackendProvider::new()),
            Arc::new(esri::EsriProvider::new()),
            Arc::new(osm::OsmProvider::new()),
            Arc::new(photon::PhotonProvider::new()),
        ])
    }

    /// The default chain configured from a `MapsConfiguration`.
    pub fn with_config(config: &MapsConfiguration) -> Self {
        Self::new(vec![
            Arc::new(backend::BackendProvider::with_config(config)),
            Arc::new(esri::EsriProvider::with_config(config)),
            Arc::new(osm::OsmProvider::with_config(config)),
            Arc::new(photon::PhotonProvider::with_config(config)),
        ])
    }

    /// Number of providers in the chain.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Provider names in fallback order.
    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// The first provider that can serve tiles for the given style, or the
    /// first provider when none matches explicitly.
    pub fn tile_provider_for(&self, style: crate::config::MapStyle) -> Option<&Arc<dyn MapProvider>> {
        self.providers
            .iter()
            .find(|p| p.supports_style(style))
            .or_else(|| self.providers.first())
    }

    /// Every provider that can serve tiles for the given style, in chain
    /// order. Tile downloads try them in order until one succeeds, so a
    /// dead backend falls back to the direct providers transparently.
    pub fn tile_providers_for(
        &self,
        style: crate::config::MapStyle,
    ) -> Vec<Arc<dyn MapProvider>> {
        self.providers
            .iter()
            .filter(|p| p.supports_style(style))
            .cloned()
            .collect()
    }

    /// Runs `call` on every provider until one succeeds.
    ///
    /// Returns [`MapsError::NoProviderAvailable`] with the last error when
    /// every provider fails or the chain is empty.
    pub fn try_each<T>(
        &self,
        mut call: impl FnMut(&dyn MapProvider) -> Result<T, MapsError>,
    ) -> Result<T, MapsError> {
        let mut last = MapsError::Provider("no provider configured".into());
        for provider in &self.providers {
            match call(provider.as_ref()) {
                Ok(value) => return Ok(value),
                Err(e) => last = e,
            }
        }
        Err(MapsError::NoProviderAvailable(Box::new(last)))
    }

    /// Search across the chain.
    pub fn search(
        &self,
        query: &str,
        near: Option<Coordinate>,
        limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        if query.trim().is_empty() {
            return Err(MapsError::InvalidQuery("query is empty".into()));
        }
        self.try_each(|p| p.search(query, near, limit))
    }

    /// Reverse geocoding across the chain.
    pub fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError> {
        self.try_each(|p| p.reverse_geocode(coordinate))
    }

    /// Place details across the chain.
    pub fn place_details(&self, place: &Place) -> Result<Place, MapsError> {
        self.try_each(|p| p.place_details(place))
    }

    /// Nearby places across the chain.
    pub fn nearby(
        &self,
        center: Coordinate,
        radius_m: u32,
        categories: &[crate::types::PlaceCategory],
        limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        if categories.is_empty() {
            return Err(MapsError::InvalidQuery(
                "at least one category is required".into(),
            ));
        }
        self.try_each(|p| p.nearby(center, radius_m, categories, limit))
    }

    /// Routing across the chain.
    pub fn route(
        &self,
        from: Coordinate,
        to: Coordinate,
        mode: TravelMode,
    ) -> Result<Route, MapsError> {
        self.try_each(|p| p.route(from, to, mode))
    }
}

impl Default for ProviderChain {
    fn default() -> Self {
        Self::default_providers()
    }
}

/// Shared HTTP session construction for all providers.
pub(crate) fn http_client(
    user_agent: &str,
    timeout_seconds: u64,
) -> networkkit::http::HttpClient {
    networkkit::http::HttpClient::with_user_agent(user_agent)
        .timeout(std::time::Duration::from_secs(timeout_seconds))
}

/// Blocking GET with URL-encoded query pairs.
pub(crate) fn get_with_query(
    client: &networkkit::http::HttpClient,
    base: &str,
    params: &[(&str, String)],
) -> Result<networkkit::http::HttpResponse, MapsError> {
    let mut url =
        url::Url::parse(base).map_err(|e| MapsError::InvalidQuery(e.to_string()))?;
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, value);
    }
    client.get(url.as_str()).send().map_err(MapsError::from)
}

/// Parses a response body as JSON.
pub(crate) fn response_json(
    resp: networkkit::http::HttpResponse,
) -> Result<serde_json::Value, MapsError> {
    resp.json().map_err(MapsError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PlaceCategory;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingProvider {
        name: &'static str,
        calls: AtomicUsize,
        fail: bool,
    }

    impl MapProvider for CountingProvider {
        fn name(&self) -> &'static str {
            self.name
        }

        fn is_available(&self) -> bool {
            !self.fail
        }

        fn search(
            &self,
            _query: &str,
            _near: Option<Coordinate>,
            _limit: usize,
        ) -> Result<Vec<Place>, MapsError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(MapsError::Provider("down".into()))
            } else {
                Ok(vec![Place::new("Result", Coordinate::ZERO)])
            }
        }

        fn reverse_geocode(&self, _coordinate: Coordinate) -> Result<Address, MapsError> {
            Err(MapsError::Provider("unsupported".into()))
        }

        fn nearby(
            &self,
            _center: Coordinate,
            _radius_m: u32,
            _categories: &[PlaceCategory],
            _limit: usize,
        ) -> Result<Vec<Place>, MapsError> {
            Err(MapsError::Provider("unsupported".into()))
        }

        fn route(
            &self,
            _from: Coordinate,
            _to: Coordinate,
            _mode: TravelMode,
        ) -> Result<Route, MapsError> {
            Err(MapsError::Provider("unsupported".into()))
        }

        fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
            format!("https://example.com/{z}/{x}/{y}.png")
        }
    }

    #[test]
    fn chain_falls_back() {
        let chain = ProviderChain::new(vec![
            Arc::new(CountingProvider {
                name: "primary",
                calls: AtomicUsize::new(0),
                fail: true,
            }),
            Arc::new(CountingProvider {
                name: "fallback",
                calls: AtomicUsize::new(0),
                fail: false,
            }),
        ]);

        let results = chain.search("berlin", None, 5).expect("fallback works");
        assert_eq!(results.len(), 1);
        assert_eq!(chain.names(), vec!["primary", "fallback"]);
    }

    #[test]
    fn chain_reports_all_failed() {
        let chain = ProviderChain::new(vec![Arc::new(CountingProvider {
            name: "only",
            calls: AtomicUsize::new(0),
            fail: true,
        })]);

        let err = chain.search("x", None, 1).unwrap_err();
        assert!(matches!(err, MapsError::NoProviderAvailable(_)));
    }

    #[test]
    fn chain_rejects_empty_query() {
        let chain = ProviderChain::new(vec![]);
        assert!(matches!(
            chain.search("  ", None, 1),
            Err(MapsError::InvalidQuery(_))
        ));
    }
}
