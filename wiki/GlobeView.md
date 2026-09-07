# GlobeView

`GlobeView` is the optional 3D globe, following Apple's `MKMapView` globe
elevation mode. The earth is a textured sphere rendered with OpenGL through
GTK's `GLArea`; the earth texture is stitched live from Web Mercator tiles
of the configured provider, so no imagery is bundled.

## Creating

```rust,no_run
use mapskit::prelude::*;

let globe = GlobeView::new(&MapsConfiguration::new().style(MapStyle::Dark));
globe.set_center(Coordinate::new(48.137, 11.575)); // Munich faces the viewer
globe.add_marker(GlobeMarker::new(Coordinate::new(48.137, 11.575), "Munich"));
globe.set_auto_rotate(true);

// globe.widget() embeds into GTK containers or UIKit views.
```

The dark style gets the dark CARTO basemap as texture, light gets the light
one and standard gets OpenStreetMap tiles.

## Earth Texture

- On creation (when `config.globe_earth_texture` is on) a 4x4 grid of
  zoom-2 tiles is fetched on a worker thread and stitched into a 1024x1024
  RGBA image.
- The result travels over an async channel to the render callback and is
  uploaded once; `reload_earth_texture()` repeats the download.
- The fragment shader samples the Mercator image by converting the sphere
  normal to lat/lon and then to Mercator UV, clamped at +/-85.0511 degrees.
- Without a texture (offline or download disabled) the shader draws a
  procedural ocean gradient with a 15 degree graticule and polar ice caps.

## Detail LOD

The base texture only resolves the whole world, so zooming in loads sharper
imagery automatically. `desired_detail_zoom(distance)` maps the camera
distance to a tile zoom level:

| Camera distance | Tile zoom | Ground resolution gain |
|---|---|---|
| `< 2.9` | 4 | 4x per axis vs base |
| `< 2.1` | 5 | 8x |
| `< 1.75` | 6 | 16x |
| `< 1.5` | 7 | 32x |

- A stitch request covers a 4x4 tile block centered on the current view
  center (`region_bounds` computes its geographic bounds). It runs on a
  worker thread; one request is in flight at a time and dragging pauses
  new requests.
- The finished image uploads as a second texture. The fragment shader uses
  it where the sphere point falls inside the region
  (`u_detail_region` = lon/mercator bounds) and falls back to the base
  texture everywhere else.
- Zooming back out past distance 2.9 drops the detail layer again.
- When the view center drifts out of the loaded region (auto-rotate,
  city jump) the same level is re-stitched around the new center.
- A failed fetch keeps the previous level and retries later.

## Rendering

| Aspect | Implementation |
|---|---|
| GL loading | glow over `glXGetProcAddress`, same loader as UIKit's ShaderView |
| Required version | GL 3.3 core (`GLArea::set_required_version(3, 3)`) |
| Geometry | 64x128 segment unit sphere, indexed triangles |
| Shading | Lambert diffuse from a fixed light direction plus blue atmosphere rim |
| Textures | Unit 0: whole-world base; unit 1: regional detail with bounds |
| Markers | `GL_POINTS` at 1.01 radius, back hemisphere clipped in the vertex shader |

Markers are capped at [`MAX_MARKERS`] = 64; further adds are ignored.

## Interaction

| Input | Action |
|---|---|
| Drag | Rotate longitude/latitude |
| Scroll | Move the camera distance by 0.25 per step (clamped `1.15..=6.0`) |
| Idle | Auto-rotate spins westward when enabled and not dragging |

Dragging rotates at a fixed 0.25 degrees per pixel relative to the center
captured at drag start. GTK reports drag offsets cumulatively from the
gesture start, so the offset maps to an absolute rotation instead of
accumulating per motion event.

## API

| Method | Description |
|---|---|
| `set_center(coord)` / `center()` | Coordinate facing the viewer |
| `set_distance(d)` / `distance()` | Camera distance, clamped to `1.15..=6.0` |
| `set_auto_rotate(bool)` / `auto_rotate()` | Idle spin switch |
| `set_markers(vec)` / `add_marker` / `clear_markers` | Marker management |
| `reload_earth_texture()` | Re-download the earth texture |
| `widget()` | The underlying `gtk::GLArea` |

Latitude clamps to `+/-89`, longitude wraps into `-180..=180`.

## Error Behavior

- A failed GL context or program link logs `[mapskit] GL init failed` and
  stops rendering that frame; the widget stays usable.
- Any tile failure during stitching returns no texture and the globe keeps
  its procedural shading; it can be retried via
  `reload_earth_texture()`.
- The idle timer holds only a weak widget reference and cancels itself once
  the view is destroyed.

## Cross References

- [Tiles.md](Tiles.md) - where texture tiles come from
- [Providers.md](Providers.md) - which provider serves the style
- [UIKit.md](UIKit.md) - embedding as `GlobeViewContent`
