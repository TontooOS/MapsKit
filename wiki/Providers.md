# Providers

MapsKit ships three map providers with automatic fallback. Every provider
implements the `MapProvider` trait; a `ProviderChain` tries them in order and
returns the first successful result, so apps keep working when one service is
down. No API keys are required.

## MapProvider

```rust
pub trait MapProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports_style(&self, style: crate::config::MapStyle) -> bool { true }
    fn is_available(&self) -> bool { true }
    fn search(&self, query: &str, near: Option<Coordinate>, limit: usize)
        -> Result<Vec<Place>, MapsError>;
    fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError>;
    fn place_details(&self, place: &Place) -> Result<Place, MapsError> { Ok(place.clone()) }
    fn nearby(&self, center: Coordinate, radius_m: u32,
        categories: &[PlaceCategory], limit: usize) -> Result<Vec<Place>, MapsError>;
    fn route(&self, from: Coordinate, to: Coordinate, mode: TravelMode)
        -> Result<Route, MapsError>;
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String;
}
```

All methods are blocking HTTP calls. Call them from worker threads, not from
the UI thread.

### Return values

| Method | Returns | Error conditions |
|---|---|---|
| `search` | Up to `limit` places | Empty query, network failure, provider error |
| `reverse_geocode` | One address | No match at the coordinate, provider error |
| `place_details` | Enriched place | Unknown id returns the place unchanged |
| `nearby` | Places sorted by distance | Empty category list, provider error |
| `route` | One route with steps | Unroutable pair, provider error |

## EsriProvider (satellite tiles)

Backed by the public ArcGIS World Imagery service
(`server.arcgisonline.com`). This provider is tile-only:

- `supports_style` returns true only for `MapStyle::Satellite`.
- `search`, `reverse_geocode`, `nearby` and `route` return a
  `MapsError::Provider` error, so the chain falls through to OSM and Photon
  for all services.
- `tile_url` uses the ArcGIS row-first order: `/tile/{z}/{y}/{x}`.

## OsmProvider (primary)

Backed by OpenStreetMap services:

| Capability | Service |
|---|---|
| Search / reverse geocoding / details | Nominatim (`nominatim.openstreetmap.org`) |
| Nearby places | Overpass API (`overpass-api.de`) |
| Routing | OSRM demo server (`router.project-osrm.org`) |
| Tiles | `tile.openstreetmap.org` (standard cartography only) |

`supports_style` returns true only for `MapStyle::Standard`. Place details
come from a Nominatim `lookup` with `extratags=1`; ids use the format
`nominatim:N123`, `nominatim:W123` or `nominatim:R123`.

## PhotonProvider (fallback)

| Capability | Service |
|---|---|
| Search / reverse geocoding / nearby | Komoot Photon (`photon.komoot.io`) |
| Routing | FOSSGIS OSRM (`routing.openstreetmap.de`) |
| Tiles (dark) | Esri dark gray canvas (`server.arcgisonline.com`, no key) |

`supports_style` returns true for `MapStyle::Dark` only; light and
standard tiles come from the primary OSM provider. (CARTO basemaps were
used here before but now require an API key, so they are no longer used.) Nearby search queries Photon per category keyword
biased to the center and filters results to the requested radius. Ids reuse
the OSM element id (`osm:N123`) when Photon provides one, else
`photon:<lat>,<lon>`.

## ProviderChain

```rust,no_run
use mapskit::{Coordinate, MapStyle, ProviderChain};

let chain = ProviderChain::default_providers();
assert_eq!(
    chain.names(),
    vec!["Esri World Imagery", "OpenStreetMap", "Photon + OSM/Esri"]
);

// Tries every provider until one succeeds.
let places = chain.search("Berlin", None, 10).expect("all failed");

// First provider that can serve the requested tile style.
let tiles = chain.tile_provider_for(MapStyle::Dark).unwrap();
let satellite = chain.tile_provider_for(MapStyle::Satellite).unwrap();
```

- `try_each(call)` runs a closure against every provider and wraps the last
  error in `MapsError::NoProviderAvailable` when all fail.
- `search` rejects whitespace-only queries with
  `MapsError::InvalidQuery`.
- `tile_provider_for(style)` falls back to the first provider when none
  matches explicitly.
- `ProviderChain::with_config(&config)` propagates user agent and timeout
  into all providers.

## Adding a Custom Provider

Implement `MapProvider` and push it onto a chain:

```rust,no_run
use std::sync::Arc;
use mapskit::{Address, Coordinate, MapsError, Place, PlaceCategory, Route, TravelMode};
use mapskit::providers::MapProvider;

struct MyProvider;

impl MapProvider for MyProvider {
    fn name(&self) -> &'static str { "MyProvider" }
    fn search(&self, _q: &str, _n: Option<Coordinate>, _l: usize)
        -> Result<Vec<Place>, MapsError> { Ok(vec![]) }
    fn reverse_geocode(&self, _c: Coordinate) -> Result<Address, MapsError>
        { Err(MapsError::Provider("unsupported".into())) }
    fn nearby(&self, _c: Coordinate, _r: u32, _k: &[PlaceCategory], _l: usize)
        -> Result<Vec<Place>, MapsError> { Ok(vec![]) }
    fn route(&self, f: Coordinate, t: Coordinate, m: TravelMode)
        -> Result<Route, MapsError> { Err(MapsError::Provider("unsupported".into())) }
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String
        { format!("https://tiles.example.com/{z}/{x}/{y}.png") }
}

let chain = mapskit::ProviderChain::new(vec![Arc::new(MyProvider)]);
```

## Usage / Example

See `examples/places_search.rs` for an end-to-end run of search, details,
reverse geocoding, nearby and routing across the default chain.

## Cross References

- [Search.md](Search.md) - what `Place` contains and how details are enriched
- [Routing.md](Routing.md) - route shapes returned by both providers
- [Tiles.md](Tiles.md) - how tile URLs are turned into cached images
