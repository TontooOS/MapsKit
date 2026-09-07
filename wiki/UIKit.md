# UIKit

MapsKit integrates with TontooUIKit through `ViewContent` wrappers, so maps
drop into any UIKit view tree exactly like a label or a button.

## MapViewContent

```rust,no_run
use uikit::prelude::*;
use mapskit::{Coordinate, MapsConfiguration, MapStyle, MapViewContent};

let config = MapsConfiguration::new().style(MapStyle::Light);
let map = MapViewContent::new(&config);
map.map_view().set_center(Coordinate::new(52.52, 13.405), 12.0);

let mut app = App::new("Maps", 900, 600);
app.set_root_view(View::new(map).with_frame(0.0, 0.0, 900.0, 600.0));
```

`MapViewContent::map_view()` returns the wrapped [`MapView`](MapView.md),
exposing the full API (camera, annotations, routes, user location,
callbacks).

## GlobeViewContent

```rust,no_run
use uikit::prelude::*;
use mapskit::{Coordinate, GlobeViewContent, MapsConfiguration};

let globe = GlobeViewContent::new(&MapsConfiguration::new());
globe.globe_view().set_center(Coordinate::new(48.137, 11.575));

let view = View::new(globe).with_frame(0.0, 0.0, 800.0, 600.0);
```

`GlobeViewContent::globe_view()` returns the wrapped
[`GlobeView`](GlobeView.md).

## Mixing with UIKit Widgets

Because both contents implement `uikit::view::ViewContent`, they compose
with stacks and other widgets like anything else:

```rust,no_run
use uikit::prelude::*;
use mapskit::{MapsConfiguration, MapViewContent};

let stack = View::new(
    VStack::new()
        .spacing(8.0)
        .child(Text::new("Maps").font_size(24.0).bold())
        .child(MapViewContent::new(&MapsConfiguration::new())),
);
```

## Threading Notes

The views already move network work to worker threads and marshal results
back onto the main loop, so no extra scheduling is needed inside a UIKit
app. Service calls made directly (`ProviderChain`) remain blocking and must
run on your own worker threads.

## Cross References

- [MapView.md](MapView.md) - 2D map behavior and callbacks
- [GlobeView.md](GlobeView.md) - 3D globe behavior
- [Ffi.md](Ffi.md) - the same integration from C
