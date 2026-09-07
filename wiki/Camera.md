# Camera

`MapCamera` is the viewport of a map view, following Apple's `MKMapCamera`:
a center coordinate, a fractional zoom level, pitch and heading.

## Creating and Moving

```rust,no_run
use mapskit::{Coordinate, MapCamera};

let mut camera = MapCamera::new(Coordinate::new(52.52, 13.405), 12.0)
    .with_pitch(30.0)
    .with_heading(90.0);

camera.zoom_in();
camera.pan_pixels(120.0, -40.0);
```

| Field | Type | Description |
|---|---|---|
| `center` | `Coordinate` | Coordinate at the viewport center |
| `zoom` | `f64` | 0 = whole world, up to `MAX_ZOOM` (19) |
| `pitch` | `f64` | Degrees, clamped to `0..=60` |
| `heading` | `f64` | Degrees clockwise from north, wrapped to `0..360` |

## Methods

| Method | Behavior |
|---|---|
| `zoom_in` / `zoom_out` | Changes the zoom by one level |
| `set_zoom(z)` | Clamps into the valid range |
| `pan_pixels(dx, dy)` | Moves the center by viewport pixels (see below) |
| `region(w, h)` | The visible `MapRegion` for a viewport size |

Pan semantics: positive `dx` moves the center west (content slides right on
screen), positive `dy` moves the center south (content slides up on screen).
Dragging therefore calls `pan_pixels(dx, -dy)`: dragging down moves content
down with the pointer.

## Regions

```rust,no_run
# use mapskit::{Coordinate, MapCamera};
let camera = MapCamera::new(Coordinate::new(48.85, 2.35), 12.0);
let region = camera.region(800.0, 600.0);
assert!(region.span.latitude_delta > 0.0);
```

`region(width_px, height_px)` returns the `MapRegion` currently visible,
computed from the ground resolution at the camera zoom.

## Defaults

The default camera centers on Berlin at zoom 11 with zero pitch and heading.
Views start from it unless `set_camera` / `set_center` is called first.

## Error Behavior

All setters clamp instead of failing: zoom stays in `0..=19`, pitch in
`0..=60`, latitude in `+/-85` and longitude wraps into `-180..=180`.

## Cross References

- [MapView.md](MapView.md) - the view driven by this camera
- [Overlays.md](Overlays.md) - fitting overlays via bounding boxes
