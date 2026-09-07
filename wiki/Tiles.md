# Tiles

MapsKit renders standard slippy map tiles (`z/x/y`, 256 px PNG). This module
provides the tile math, network fetching and an on-disk cache.

## Tile Math

```rust,no_run
use mapskit::{Coordinate, tiles};

let berlin = Coordinate::new(52.52, 13.405);

// Fractional tile coordinates.
let (fx, fy) = tiles::tile_for(berlin, 12);

// Integer tile containing the coordinate.
let key = tiles::tile_containing(berlin, 3);
assert_eq!(key, tiles::TileKey::new(3, 4, 2));

// North-west corner and center of a tile.
let corner = key.north_west();
let center = key.center();
```

| Function | Description |
|---|---|
| `tile_for(coord, z)` | Fractional tile position |
| `tile_containing(coord, z)` | Integer `TileKey` under the coordinate |
| `coord_for_tile(x, y, z)` | North-west corner coordinate |
| `clamp_zoom(zoom)` | Clamps into `0..=19` (`MAX_ZOOM`) |
| `meters_per_pixel(zoom, lat)` | Ground resolution at a latitude |

`TileKey::neighbors()` returns the four adjacent tiles; edges outside the
world return `None`.

## Fetching

```rust,no_run
# use mapskit::{tiles, MapsConfiguration, ProviderChain};
# fn demo(config: &MapsConfiguration) {
let chain = ProviderChain::default_providers();
let provider = chain.tile_provider_for(config.style).unwrap();
let source = tiles::cache_source_key(provider.as_ref());
let cache = tiles::TileCache::new(config, "tiles", &source);
let key = tiles::TileKey::new(10, 550, 335);
let png: Vec<u8> = tiles::fetch_tile(
    provider.as_ref(), &cache, key,
    &config.user_agent, config.timeout_seconds,
).expect("tile fetch failed");
# }
```

`fetch_tile` is blocking and must run on a worker thread. It checks the
cache first, downloads through the provider on miss, validates the image
magic number (PNG for OSM/CARTO, JPEG for Esri satellite), and stores the
result.

## Caching

`cache_source_key(provider)` derives a stable namespace from the provider
name plus a hash of its sample tile URL, so style variants that serve
different imagery (CARTO Voyager vs Dark Matter) never share files. The
`TileCache` writes PNG files to
`<cache_dir>/tiles/<provider>-<source>/<z>/<x>/<y>.png`. The default cache directory
is `$XDG_CACHE_HOME/tontoos/mapskit` or `~/.cache/tontoos/mapskit`; override
it with [`MapsConfiguration`](../wiki/MAIN.md)::`cache_directory`.
Corrupt or missing entries simply re-download.

## Which Provider Serves Tiles

Providers declare the styles they can serve via
`MapProvider::supports_style`:

| Style | Serving provider |
|---|---|
| `MapStyle::Standard` | OpenStreetMap standard cartography |
| `MapStyle::Light` | CARTO Voyager (colorful, Apple Maps-like) |
| `MapStyle::Dark` | CARTO `dark_all` |
| `MapStyle::Satellite` | Esri World Imagery |

`ProviderChain::tile_provider_for(style)` picks the first matching provider;
the map views call this whenever the style changes and evict their in-memory
tiles.

## Error Behavior

- Non-2xx responses become `MapsError::Tile` with status and tile id.
- Responses that do not start with the PNG magic number become
  `MapsError::Tile("response is not a PNG image")`.
- Network failures surface as `MapsError::Network`.

## Cross References

- [MapView.md](MapView.md) - how visible tiles are requested and drawn
- [GlobeView.md](GlobeView.md) - how tiles are stitched into the earth texture
- [Providers.md](Providers.md) - tile URL templates per provider
