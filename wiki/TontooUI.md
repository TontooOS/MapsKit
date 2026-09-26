# TontooUI

MapsKit integrates with TontooUI natively: `MapView` and `GlobeView`
implement `tontooui::elements::View` directly, so maps drop into any
`VStack` / `HStack` / `ZStack` exactly like a label or a button.

## MapView

```rust,no_run
use tontooui::elements::{View, VStack};
use mapskit::{Coordinate, MapsConfiguration, MapStyle, MapView};

let mut map = MapView::new(&MapsConfiguration::new().style(MapStyle::Light));
map.set_center(Coordinate::new(52.52, 13.405), 12.0);

let stack = VStack::new().child(map);
```

`MapView` exposes the full API (camera, annotations, routes, user
location, callbacks) and is `Clone`: clones share the same model through
an `Arc<Mutex<..>>`, so worker threads and button callbacks can drive the
map while the shell draws it.

## GlobeViewContent

```rust,no_run
use tontooui::elements::{View, VStack};
use mapskit::{Coordinate, GlobeView, MapsConfiguration, MapViewContent};

let mut globe = GlobeView::new(&MapsConfiguration::new());
globe.set_center(Coordinate::new(48.137, 11.575));

let stack = VStack::new().child(globe);
```

## MapViewContent Wrappers

`MapViewContent` / `GlobeViewContent` wrap a view for trees that expect an
owned content object. They forward the `View` trait to the inner view:

```rust,no_run
use tontooui::elements::{View, VStack};
use mapskit::{MapsConfiguration, MapViewContent};

let stack = VStack::new().child(MapViewContent::new(&MapsConfiguration::new()));
```

Prefer embedding `MapView` / `GlobeView` directly when possible.

## Event Forwarding

The host `App` must forward pointer input so pan and zoom work:

```rust,no_run
# use mapskit::MapView;
# use tontooui::renderer::window::{App, Key};
# struct Demo { map: MapView }
# impl App for Demo {
fn mouse_down(&mut self, x: f64, y: f64) { self.map.mouse_down(x, y); }
fn mouse_up(&mut self, x: f64, y: f64) { self.map.mouse_up(x, y); }
fn mouse_move(&mut self, x: f64, y: f64) { self.map.set_hover(x as f32, y as f32); }
fn mouse_wheel(&mut self, dx: f64, dy: f64) { self.map.mouse_wheel(dx, dy); }
# fn draw(&mut self, s: &mut vello::Scene, f: &mut tontooui::renderer::text::FontSystem, i: &mut tontooui::renderer::images::ImageLoader<'_>, v: tontooui::renderer::window::Viewport, t: f64) {}
# }
```

Stacks forward these calls to children automatically when the map is
embedded in a `VStack` / `HStack`.

## Threading Notes

The views already move network work to worker threads and write results
into shared state; the TontooUI shell redraws continuously, so no extra
scheduling is needed. Service calls made directly (`ProviderChain`)
remain blocking and must run on your own worker threads.

## Cross References

- [MapView.md](MapView.md) - 2D map behavior and callbacks
- [GlobeView.md](GlobeView.md) - globe behavior
- [Ffi.md](Ffi.md) - the same integration from C
