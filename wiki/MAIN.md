# TontooMapsKit -- Wiki

TontooMapsKit is the maps framework for TontooOS. It follows Apple's MapKit
design philosophy: an interactive `MapView`, a `MapCamera` viewport object,
annotations and overlays, place search with details, forward/reverse
geocoding, turn-by-turn routing, an optional 3D `GlobeView` and a C FFI.
Three providers are built in with automatic fallback (Esri satellite tiles,
OpenStreetMap primary, Photon + CARTO fallback), so no API keys are required.

- Repository: tontoo-os/TontooLibs/MapsKit
- License: TCL
- Version: 26.1.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| Providers | [Providers.md](Providers.md) | `MapProvider` trait and the fallback chain |
| Search | [Search.md](Search.md) | Place search with details and nearby POIs |
| Geocoding | [Geocoding.md](Geocoding.md) | Reverse geocoding into structured addresses |
| Routing | [Routing.md](Routing.md) | Turn-by-turn routes via OSRM |
| Tiles | [Tiles.md](Tiles.md) | Slippy map tile math, fetching and caching |
| Camera | [Camera.md](Camera.md) | The `MapCamera` viewport object |
| Overlays | [Overlays.md](Overlays.md) | Annotations, polylines, polygons, circles |
| MapView | [MapView.md](MapView.md) | The interactive 2D map view |
| GlobeView | [GlobeView.md](GlobeView.md) | The optional 3D globe view |
| FFI | [Ffi.md](Ffi.md) | C API and `Headers/mapskit.h` |
| UIKit | [UIKit.md](UIKit.md) | Embedding in UIKit apps via `ViewContent` |

## Quick Start

```rust,no_run
use uikit::prelude::*;
use mapskit::prelude::*;

fn main() {
    let config = MapsConfiguration::new().style(MapStyle::Light);
    let map = MapViewContent::new(&config);
    map.map_view().set_center(Coordinate::new(52.52, 13.405), 12.0);

    let mut app = App::new("Maps", 900, 600);
    app.set_root_view(View::new(map).with_frame(0.0, 0.0, 900.0, 600.0));
    app.run();
}
```

Services without UI:

```rust,no_run
use mapskit::{Coordinate, ProviderChain};

let chain = ProviderChain::default_providers();
let results = chain.search("Brandenburger Tor", None, 5).expect("search");
println!("{}", results[0].name);
```

See [Providers.md](Providers.md), [MapView.md](MapView.md) and
[UIKit.md](UIKit.md) for details.

## Architecture

```
MapsConfiguration (style, POI filter, cache dir, user agent)
  |
  +-- ProviderChain          (ordered fallback over MapProvider)
  |     +-- EsriProvider     (Esri World Imagery satellite tiles)
  |     +-- OsmProvider      (Nominatim + Overpass + OSRM + OSM tiles)
  |     +-- PhotonProvider   (Photon + FOSSGIS OSRM + CARTO tiles)
  |
  +-- MapView                (GTK4 DrawingArea, Cairo tiles, pan/zoom,
  |                           satellite layer pill bottom left)
  |     +-- TileCache        (on-disk PNG cache)
  |     +-- MapCamera        (center / zoom / pitch / heading)
  |     +-- Overlays         (annotations, polylines, polygons, circles)
  |     +-- user location    (CoreLocation integration)
  +-- GlobeView              (GTK4 GLArea + glow, textured sphere)
  +-- Services               (search / geocode / route, blocking)
  |
  +-- FFI                    (C ABI, Headers/mapskit.h)
  +-- MapViewContent / GlobeViewContent   (uikit ViewContent wrappers)
```

## Performance Notes

- All provider calls are blocking (NetworkKit `networkkit::http`). Call them from
  worker threads; the views already do this internally for tiles, the earth
  texture and user location.
- Tiles are cached on disk under `$XDG_CACHE_HOME/tontoos/mapskit/tiles/`
  (or the configured `cache_directory`) keyed by provider name and style.
- The map view keeps only tiles of the current zoom level in memory; older
  levels are evicted on zoom change.
- The globe stitches one 4x4 grid of zoom-2 tiles (1024x1024 px) per earth
  texture download and reuses it until `reload_earth_texture()` is called.

## Changelog

- 2026-09-26: HTTP transport moved to NetworkKit (`networkkit::http` with a
  shared `HttpClient` session per provider, query building via the `url`
  crate). The `reqwest` dependency is removed.
- 2026-08-23: GlobeView detail LOD: zooming in now stitches sharper 4x4
  tile blocks (zoom 4-7) around the view center and blends them over the
  base texture; the camera can go closer (distance clamp `1.15..=6.0`) and
  the detail layer follows the center when it drifts. New
  `examples/globe_demo.rs`.
- 2026-08-23: Fixed satellite imagery: Esri World Imagery serves JPEG, but
  the tile fetcher only accepted the PNG magic number, so every satellite
  tile failed and the view showed the offline badge.
- 2026-08-23: The light street layer now uses CARTO Voyager tiles, whose
  colorful Apple-Maps-like cartography (green parks, blue water, orange
  roads) replaces the pale Positron basemap. Tile cache directories now
  include a per-source key (`cache_source_key`), fixing stale tiles when
  switching between light and dark imagery.
- 2026-08-23: Satellite layer: new `MapStyle::Satellite` served by the tile
  only `EsriProvider` (ArcGIS World Imagery), a pill toggle at the bottom
  left of MapView plus `set_satellite(bool)` / `is_satellite()`, Esri
  attribution strings in the language files and FFI `"satellite"` style.
- 2026-08-23: Fixed drag input in MapView and GlobeView: GTK reports
  cumulative drag offsets, which were re-applied per motion event and made
  panning/rotating accelerate. Both views now apply deltas once (MapView)
  or rotate absolutely from the drag-start center (GlobeView).
- 2026-08-21: Initial wiki, MapsKit crate, provider chain (OpenStreetMap +
  Photon/CARTO), search/geocoding/nearby/routing services, 2D MapView with
  pan/zoom/overlays/user location, 3D GlobeView, C FFI, UIKit integration,
  examples and language files.
