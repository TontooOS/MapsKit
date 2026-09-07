# Overlays

Overlays draw geometry on top of the base map: annotations (`MKPointAnnotation`),
polylines (`MKPolyline`), polygons (`MKPolygon`) and circles (`MKCircle`).

## Annotations

```rust,no_run
use mapskit::{Annotation, Coordinate, Place};

let pin = Annotation::new(Coordinate::new(52.5163, 13.3777), "Brandenburg Gate")
    .subtitle("Pariser Platz 1, Berlin")
    .glyph("mappin");

// Build one from a Place: id, title and address subtitle are filled in.
# let place = Place::new("Cafe", Coordinate::ZERO).with_id("osm:N1");
let from_place = Annotation::from_place(&place);
```

| Field | Type | Description |
|---|---|---|
| `id` | `String` | Stable id for removal (place id or generated) |
| `coordinate` | `Coordinate` | Pin position |
| `title` | `String` | Label drawn next to the pin |
| `subtitle` | `Option<String>` | Reserved for detail callouts |
| `glyph` | `Option<String>` | SF Symbol name for the pin icon |
| `selected` | `bool` | Drawn highlighted |

Pins render as accent-colored dots with a label plate; the user-location dot
is drawn separately by the view.

## Polylines

```rust
# use mapskit::{Coordinate, Polyline};
let line = Polyline::new(
    vec![Coordinate::new(0.0, 0.0), Coordinate::new(1.0, 1.0)],
    "#FF6B2B",
)
.width(4.0);
```

Colors are CSS strings (`#RRGGBB` or `rgba(r,g,b,a)`). `Polyline::for_route`
builds an accent polyline from a [`Route`](Routing.md) with width 4.

## Polygons and Circles

```rust,no_run
use mapskit::{CircleOverlay, Coordinate, Polygon};

let area = Polygon::new(vec![
    Coordinate::new(52.51, 13.37),
    Coordinate::new(52.52, 13.38),
    Coordinate::new(52.51, 13.39),
])
.fill("rgba(255,107,43,0.25)")
.stroke("#FF6B2B");

let radius = CircleOverlay::new(Coordinate::new(52.52, 13.40), 500.0)
    .fill("rgba(0,229,255,0.20)")
    .stroke("#00E5FF");
```

An empty fill or stroke color string skips that pass when drawing.

## The Overlay Enum

All three wrap into `Overlay`, which is what map views store:

```rust,no_run
# use mapskit::{CircleOverlay, Coordinate, Overlay, Polyline};
let overlay: Overlay = Polyline::new(vec![], "#fff").into();
match overlay {
    Overlay::Polyline(_) => {}
    Overlay::Polygon(_) => {}
    Overlay::Circle(_) => {}
}
```

Every overlay exposes `bounding_box() -> Option<BoundingBox>` used to fit
the viewport; it returns `None` for empty point lists.

## Error Behavior

- Polylines with fewer than two points and polygons with fewer than three
  points are skipped by the renderer.
- Unparseable color strings fall back to the TontooOS accent color at full
  alpha.
- Circle radii below 1 m are clamped to 1 m; on-screen radii below 2 px are
  drawn at 2 px so they stay visible.

## Cross References

- [MapView.md](MapView.md) - add/remove overlays and annotation taps
- [Routing.md](Routing.md) - routes rendered as polylines
- [Search.md](Search.md) - places converted to annotations
