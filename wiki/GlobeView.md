# GlobeView

`GlobeView` is the optional globe, following Apple's `MKMapView` globe
elevation mode. The earth is rendered as a shaded 2D disc with Vello
inside TontooUI: an ocean gradient with polar caps, a 15 degree graticule
and accent marker dots on the front hemisphere. No OpenGL context is
required, so the globe works on every TontooUI target.

The view implements `tontooui::elements::View` directly and embeds into
any stack. It is `Clone`: clones share one model through an
`Arc<Mutex<..>>`. The host `App` must forward `mouse_down` / `mouse_up` /
`mouse_wheel` and `mouse_move` (as `set_hover`); see
[TontooUI.md](TontooUI.md).

## Creating

```rust,no_run
use mapskit::prelude::*;

let globe = GlobeView::new(&MapsConfiguration::new().style(MapStyle::Dark));
globe.set_center(Coordinate::new(48.137, 11.575)); // Munich faces the viewer
globe.add_marker(GlobeMarker::new(Coordinate::new(48.137, 11.575), "Munich"));
globe.set_auto_rotate(true);

// The globe implements tontooui::elements::View and embeds into any stack.
let stack = VStack::new().child(globe);
```

`config.globe_earth_texture` is accepted for compatibility but currently
reserved: the renderer draws a procedural ocean and needs no downloaded
imagery.

## Rendering

| Aspect | Implementation |
|---|---|
| Backend | Vello 2D scene inside TontooUI (no GL context) |
| Disc | Ocean fill with polar caps, night-side shade and rim light |
| Graticule | Parallels and meridians every 15 degrees, front hemisphere only |
| Projection | Orthographic: the center coordinate faces the viewer |
| Markers | Accent dots with halo on the front hemisphere (`z > 0.10`), plus labels |

Markers are capped at `MAX_MARKERS` = 64; further adds are ignored.

## Interaction

| Input | Action |
|---|---|
| Drag | Rotate longitude/latitude at 0.25 degrees per pixel |
| Scroll | Move the camera distance by 0.25 per notch (clamped `1.15..=6.0`) |
| Idle | Auto-rotate spins westward 0.18 degrees per frame when enabled and not dragging |

## API

| Method | Description |
|---|---|
| `set_center(coord)` / `center()` | Coordinate facing the viewer |
| `set_distance(d)` / `distance()` | Camera distance, clamped to `1.15..=6.0` |
| `set_auto_rotate(bool)` / `auto_rotate()` | Idle spin switch |
| `set_markers(vec)` / `add_marker` / `clear_markers` / `markers()` | Marker management |
| `reload_earth_texture()` | Compatibility no-op (procedural ocean needs no download) |
| `rect()` | Placed rect in logical px |

Latitude clamps to `+/-89`, longitude wraps into `-180..=180`.

## Error Behavior

- The renderer cannot fail: with no downloaded imagery there is nothing
  to time out, so the globe always draws.
- `set_markers` truncates to `MAX_MARKERS`; `add_marker` ignores markers
  past the cap.

## Cross References

- [Providers.md](Providers.md) - which provider serves each style
- [TontooUI.md](TontooUI.md) - embedding as a `View` or `GlobeViewContent`
