# Routing

MapsKit computes turn-by-turn routes between two coordinates. Both providers
speak the OSRM HTTP protocol; the chain falls back automatically.

## Computing a Route

```rust,no_run
use mapskit::{Coordinate, ProviderChain, TravelMode};

let chain = ProviderChain::default_providers();
let from = Coordinate::new(52.5163, 13.3777); // Brandenburg Gate
let to = Coordinate::new(52.5219, 13.4132);   // Alexanderplatz

let route = chain.route(from, to, TravelMode::Walking).expect("no route");
println!("{} / {}", route.distance_text(), route.duration_text());
for step in &route.steps {
    println!(" - {}", step.instruction);
}
```

## Travel Modes

| Mode | OSRM profile (primary) | FOSSGIS profile (fallback) |
|---|---|---|
| `TravelMode::Driving` | `driving` on `router.project-osrm.org` | `routed-car` |
| `TravelMode::Walking` | `foot` | `routed-foot` |
| `TravelMode::Cycling` | `bike` | `routed-bike` |

The fallback provider prefixes its base URL with the FOSSGIS profile path,
so both providers share one request builder (`fetch_osrm_route`).

## The Route Type

| Field | Type | Description |
|---|---|---|
| `distance_m` | `f64` | Total distance in meters |
| `duration_s` | `f64` | Estimated travel time in seconds |
| `geometry` | `Vec<Coordinate>` | Full polyline of the route |
| `steps` | `Vec<RouteStep>` | Turn-by-turn instructions |
| `source` | `String` | Provider name that computed the route |

Each `RouteStep` carries `instruction`, `distance_m`, `duration_s` and the
maneuver `coordinate`.

Instructions are built from OSRM maneuver types and modifiers, e.g. `Turn
left onto Hauptstrasse`, `Take the roundabout`, `Arrive at your destination`.

## Display Formatting

```rust,no_run
# use mapskit::{format_distance, format_duration};
assert_eq!(format_distance(850.0), "850 m");
assert_eq!(format_distance(12_400.0), "12.4 km");
assert_eq!(format_duration(720.0), "12 min");
assert_eq!(format_duration(4980.0), "1 h 23 min");
```

## Showing a Route on the Map

```rust,no_run
# use mapskit::prelude::*;
# fn demo(map: &MapView, route: &Route) {
map.display_route(route);
# }
```

`MapView::display_route` clears overlays, draws an accent-colored
[`Polyline`](Overlays.md) and fits the viewport to the route bounding box.
See [MapView.md](MapView.md).

## Error Behavior

- An OSRM code other than `Ok` becomes `MapsError::Provider`.
- Missing routes or geometry become `MapsError::Parse`.
- All providers failing wraps the last error in
  `MapsError::NoProviderAvailable`.

## Cross References

- [Providers.md](Providers.md) - endpoints and profiles per provider
- [Overlays.md](Overlays.md) - the polyline drawn for routes
- [MapView.md](MapView.md) - displaying and fitting routes
