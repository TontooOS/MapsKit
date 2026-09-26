# MapView

`MapView` is the interactive 2D map view, following Apple's `MKMapView`. It
renders slippy map tiles with Vello inside TontooUI, supports pan and zoom,
draws overlays and annotations, shows the user location via CoreLocation
and displays routes.

The view implements `tontooui::elements::View` directly and embeds into any
stack. It is `Clone`: clones share one model through an `Arc<Mutex<..>>`,
so worker threads and button callbacks can drive it while the shell draws
it. The host `App` must forward `mouse_down` / `mouse_up` / `mouse_wheel`
and `mouse_move` (as `set_hover`); see [TontooUI.md](TontooUI.md).

## Creating

```rust,no_run
use mapskit::prelude::*;

let map = MapView::new(&MapsConfiguration::new().style(MapStyle::Light));
map.set_center(Coordinate::new(52.52, 13.405), 12.0);
map.add_annotation(Annotation::new(Coordinate::new(52.52, 13.405), "Berlin"));

// The map implements tontooui::elements::View and embeds into any stack.
let stack = VStack::new().child(map);
```

## Gestures

| Input | Action |
|---|---|
| Drag | Pan (content follows the pointer) |
| Scroll | Zoom around the hover anchor (one notch is one level) |
| Tap on a pin | Fires the annotation-tapped callback |
| Tap on layer pill (bottom left) | Switch between satellite imagery and the base style |

Panning tracks the hover position between press and release and applies
only the delta since the last motion event, so panning speed matches the
pointer one-to-one and never accelerates during long drags.

The layer pill at the bottom left shows the name of the layer it switches to
("Satellite" in map mode, "Standard" in satellite mode) and uses the
localized strings `mapskit.map.satellite` / `mapskit.map.standard`. The
attribution badge updates per layer (`mapskit.map.attribution.esri` while
satellite imagery is shown). A press on the pill never starts a pan or an
annotation tap.

## Camera API

| Method | Description |
|---|---|
| `camera()` / `set_camera` | Read or replace the whole camera |
| `set_center(coord, zoom)` | Center at a zoom level |
| `region()` | Currently visible `MapRegion` |
| `set_region(region)` | Fit to a region |
| `fit_bounds(bbox)` | Fit to a bounding box |

Zoom is fractional: tiles render at the nearest integer level and are scaled
by the fractional remainder, giving smooth zooming. The zoom-around-anchor
logic keeps the coordinate under the cursor fixed while scrolling.

## Layer Switching

```rust,no_run
# use mapskit::MapView;
# fn demo(map: &MapView) {
map.set_satellite(true); // Esri World Imagery tiles
assert!(map.is_satellite());
map.set_satellite(false); // Back to the configured base style
# }
```

- `set_satellite(enabled)`: switches between satellite imagery and the base
  style from the configuration. Cached and pending tiles of the old layer
  are dropped, so no stale tiles of the previous provider are shown.
- `is_satellite()`: whether satellite imagery is currently rendered.

The same switch is available as a pill button at the bottom left of the map.

## Annotations and Overlays

```rust,no_run
# use mapskit::prelude::*;
# fn demo(map: &MapView) {
map.add_annotation(Annotation::new(Coordinate::ZERO, "Pin"));
map.remove_annotation("osm:N1");
map.clear_annotations();

map.add_overlay(Polyline::new(vec![], "#FF6B2B"));
map.add_overlay(CircleOverlay::new(Coordinate::ZERO, 300.0));
map.clear_overlays();
# }
```

`show_place(place)` clears annotations, drops one for the place and centers
at zoom 14 or higher. `display_route(route)` draws the route polyline and
fits the viewport; see [Routing.md](Routing.md).

## Callbacks

```rust,no_run
# use mapskit::{MapCamera, MapView};
# fn demo(map: &MapView) {
map.on_annotation_tapped(|annotation| {
    println!("tapped: {}", annotation.title);
});
map.on_camera_changed(|camera: &MapCamera| {
    println!("center: {}", camera.center);
});
# }
```

Callbacks run on the calling thread during input handling. They are cloned
out of the lock before invocation, so a callback may call back into the
view (e.g. `clear_annotations`) without deadlocking. Callbacks must be
`Send + Sync` because the model is shared with worker threads.

## User Location

```rust,no_run
# use mapskit::prelude::*;
# fn demo(map: &MapView) {
// Query CoreLocation on a worker thread; draws the blue dot and centers.
map.show_user_location();

// Or set it manually:
map.set_user_location(Coordinate::new(52.52, 13.405));
let shown = map.user_location();
map.hide_user_location();
# }
```

CoreLocation failures are only logged when `MAPSKIT_DEBUG=1`.

## Tile Loading Behavior

- Visible tiles are requested during the draw pass; missing ones spawn one
  worker thread each.
- Threads fetch through the disk cache and write raw bytes into shared
  state. The TontooUI shell redraws continuously, so tiles appear on the
  next frame with no explicit invalidation.
- `ImageLoader::raster` decodes and uploads each tile once per cache key
  (`mapskit/<provider>/<source>/<z>/<x>/<y>`).
- Switching the base style drops all cached tiles of the old layer.
- While loading, placeholder rectangles tinted per style are drawn; after a
  failure an offline badge appears top-left.

## Background Colors

Per TontooOS design rules the view background follows the style: dark and
satellite use `#1b2022`, light and standard use `#ffffff`. Text (labels,
badges, pills) renders through the system font stack, so SF Pro resolves
on TontooOS with automatic fallback elsewhere.

## Cross References

- [Camera.md](Camera.md) - the viewport object behind the gestures
- [Overlays.md](Overlays.md) - overlay types and drawing details
- [Tiles.md](Tiles.md) - fetching and caching behavior
- [TontooUI.md](TontooUI.md) - embedding as a `View` or `MapViewContent`
