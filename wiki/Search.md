# Search

Place search returns `Place` values (the MapKit `MKMapItem` equivalent):
name, category, coordinate, address and extra details. Search works through
the [`ProviderChain`](Providers.md), so the Photon fallback answers
automatically when Nominatim is unreachable.

## Searching

```rust,no_run
use mapskit::{Coordinate, ProviderChain};

let chain = ProviderChain::default_providers();
let near = Coordinate::new(52.52, 13.405);

// Bias results towards an area without hard-bounding them.
let places = chain.search("cafe", Some(near), 10).expect("search failed");
for place in &places {
    println!("{} at {}", place.name, place.coordinate);
}
```

- Whitespace-only queries return `MapsError::InvalidQuery`.
- The optional reference coordinate biases ranking (Nominatim viewbox,
  Photon lat/lon bias).
- `limit` is clamped to 1..=50.

## The Place Type

| Field | Type | Description |
|---|---|---|
| `id` | `String` | Stable provider id (`osm:N123`, `nominatim:R45`) |
| `name` | `String` | Display name |
| `category` | `PlaceCategory` | One of 28 categories (see below) |
| `coordinate` | `Coordinate` | WGS 84 position |
| `address` | `Address` | Street, city, postcode, country, formatted line |
| `info` | `PlaceInfo` | Phone, website, opening hours, cuisine, ... |
| `distance_m` | `Option<f64>` | Distance to the query reference, when known |

## Place Categories

`PlaceCategory` maps between user-facing categories and OpenStreetMap tags:

```rust,no_run
use mapskit::{Place, PlaceCategory};
use mapskit::types::Address;

let place = Place::new("Cafe Central", mapskit::Coordinate::new(48.2, 16.37))
    .with_id("osm:N1")
    .with_category(PlaceCategory::Cafe)
    .with_address(Address { city: Some("Vienna".into()), ..Default::default() });

assert_eq!(
    PlaceCategory::from_osm_tag("amenity", "cafe"),
    Some(PlaceCategory::Cafe)
);
```

Categories cover restaurants, cafes, bars, hotels, shops, health, finance,
mobility, education, culture, leisure and public services. Each category
carries its Overpass tag filters and a Photon keyword for the fallback.

## Place Details

Details enrich a place with contact and amenity information:

```rust,no_run
# let chain = mapskit::ProviderChain::default_providers();
# let places: Vec<mapskit::Place> = Vec::new();
# if let Some(first) = places.first() {
let details = chain.place_details(first).expect("details failed");
if let Some(hours) = &details.info.opening_hours {
    println!("Opening hours: {hours}");
}
# }
```

The primary provider looks the id up via Nominatim with `extratags=1`.
Places whose id does not start with `nominatim:` are returned unchanged.
Overpass-based `nearby` results already carry their details inline.

## Nearby Places

```rust,no_run
# use mapskit::{Coordinate, ProviderChain, PlaceCategory};
# let chain = ProviderChain::default_providers();
let cafes = chain.nearby(
    Coordinate::new(52.5163, 13.3777),
    800,
    &[PlaceCategory::Cafe],
    5,
).expect("nearby failed");
for cafe in &cafes {
    println!("{} - {} m", cafe.name,
        cafe.distance_m.map(|d| d as u32).unwrap_or(0));
}
```

Results are sorted by distance from the center. An empty category slice
returns `MapsError::InvalidQuery`.

## Error Behavior

- All providers failing wraps the last error in
  `MapsError::NoProviderAvailable`.
- HTTP failures surface as `MapsError::Network` or `MapsError::Provider`
  with the status code.
- Malformed responses surface as `MapsError::Parse`.

## Cross References

- [Providers.md](Providers.md) - provider implementations and fallback
- [Geocoding.md](Geocoding.md) - reverse geocoding into addresses
