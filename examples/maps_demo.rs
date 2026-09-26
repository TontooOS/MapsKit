//! Interactive TontooMapsKit demo (TontooUI).
//!
//! A map with a floating glass toolbar: a search field plus locate/route
//! actions hovering directly over the map. Search and routing run on
//! worker threads so the UI stays responsive; results write straight into
//! the shared map model.
//!
//! Run with: `cargo run --example maps_demo`
//!
//! Controls: click the map and drag to pan, scroll to zoom, click the
//! Satellite/Standard pill to switch layers. Type a query and press Enter
//! to search from your location (nearest first), then the route action
//! for driving directions to the nearest result.

use mapskit::prelude::*;
use std::sync::{Arc, Mutex};
use tontooui::elements::{BasicToolbar, SearchField, Titlebar, ToolbarItem, TrafficAction, View};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;

/// Latest search results, shared with the toolbar callback.
type SharedResults = Arc<Mutex<Vec<Place>>>;

/// Origin for search and routing: the known user dot, else a fresh
/// CoreLocation fix (which also sets the dot and refreshes the map cache),
/// else the map center. Returns the coordinate and whether it fell back to
/// the map center.
fn locate_origin(map: &MapView) -> (Coordinate, bool) {
    let mut from_center = false;
    let origin = map
        .user_location()
        .or_else(|| {
            corelocation::get_location().ok().map(|loc| {
                let c = Coordinate::new(loc.coordinates.latitude, loc.coordinates.longitude);
                map.set_user_location(c);
                c
            })
        })
        .unwrap_or_else(|| {
            from_center = true;
            map.camera().center
        });
    (origin, from_center)
}

/// Sorts places by distance to `origin` (nearest first).
fn sort_by_distance(places: &mut Vec<Place>, origin: Coordinate) {
    places.sort_by(|a, b| {
        let da = a.coordinate.distance_to(&origin);
        let db = b.coordinate.distance_to(&origin);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Routes from the user location to the nearest of `places` and displays
/// the result on `map`. Blocking — call from a worker thread.
fn route_to_nearest(map: &MapView, places: &[Place]) {
    if places.is_empty() {
        println!("[maps_demo] search for a place first (type + Enter)");
        return;
    }
    let (origin, from_center) = locate_origin(map);
    let Some(dest) = places.iter().min_by(|a, b| {
        let da = a.coordinate.distance_to(&origin);
        let db = b.coordinate.distance_to(&origin);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        return;
    };
    if from_center {
        println!("[maps_demo] no location available, routing from map center");
    }
    println!("[maps_demo] routing to nearest result: {}", dest.name);
    match ProviderChain::default_providers().route(origin, dest.coordinate, TravelMode::Driving) {
        Ok(route) => {
            println!(
                "[maps_demo] route {} / {} via {}",
                route.distance_text(),
                route.duration_text(),
                route.source
            );
            for step in route.steps.iter().take(5) {
                println!("   - {}", step.instruction);
            }
            map.display_route(&route);
        }
        Err(e) => eprintln!("[maps_demo] route failed: {e}"),
    }
}

struct MapsDemo {
    bar: Titlebar,
    search: SearchField,
    toolbar: BasicToolbar,
    map: MapView,
    results: SharedResults,
    /// Floating overlay rect (search + toolbar) for hit-testing, so presses
    /// on the glass never start a map drag.
    overlay: (f32, f32, f32, f32),
    watcher: ThemeWatcher,
    focused: bool,
    bg: Color,
    command: Option<WindowCommand>,
    cursor: (f64, f64),
}

impl MapsDemo {
    fn new() -> Self {
        let config = MapsConfiguration::new()
            .style(MapStyle::Light)
            .shows_points_of_interest(true);
        let map = MapView::new(&config);
        map.set_center(Coordinate::new(52.5163, 13.3777), 13.0);
        map.on_annotation_tapped(|annotation| {
            println!("[maps_demo] tapped: {}", annotation.title);
        });

        let search = SearchField::new(mapskit::localization::t_or(
            "mapskit.search.placeholder",
            "Search places and addresses",
        ));

        // Floating glass toolbar: locate + route. The cloned map and
        // results share their models, so the worker thread updates the UI
        // directly and the shell picks it up on the next frame.
        let action_map = map.clone();
        let action_results: SharedResults = Arc::new(Mutex::new(Vec::new()));
        let toolbar = BasicToolbar::from_items(vec![
            ToolbarItem::icon("location.fill"),
            ToolbarItem::icon("arrow.turn.up.right"),
        ])
        .on_action({
            let map = action_map.clone();
            let results = action_results.clone();
            move |index| match index {
                0 => map.show_user_location(),
                1 => {
                    let map = map.clone();
                    let results = results.clone();
                    std::thread::spawn(move || {
                        let places = results.lock().map(|r| r.clone()).unwrap_or_default();
                        route_to_nearest(&map, &places);
                    });
                }
                _ => {}
            }
        });

        Self {
            bar: Titlebar::new("TontooMapsKit"),
            search,
            toolbar,
            map,
            results: action_results,
            overlay: (0.0, 0.0, 0.0, 0.0),
            watcher: ThemeWatcher::new(),
            focused: true,
            bg: tontooui::renderer::window::BACKGROUND,
            command: None,
            cursor: (0.0, 0.0),
        }
    }

    fn run_search(&self) {
        let query = self.search.text_value().to_string();
        if query.trim().is_empty() {
            return;
        }
        let map = self.map.clone();
        let results = self.results.clone();
        std::thread::spawn(move || {
            // Search from the user location: "Aldi" finds the Aldi stores
            // around you, sorted nearest first.
            let (origin, from_center) = locate_origin(&map);
            if from_center {
                println!("[maps_demo] no location available, searching from map center");
            }
            println!(
                "[maps_demo] searching near {:.4},{:.4}",
                origin.latitude, origin.longitude
            );
            match ProviderChain::default_providers().search(&query, Some(origin), 10) {
                Ok(mut places) => {
                    if places.is_empty() {
                        println!(
                            "{}",
                            mapskit::localization::t_or(
                                "mapskit.search.no_results",
                                "No results found."
                            )
                        );
                        return;
                    }
                    sort_by_distance(&mut places, origin);
                    for place in &places {
                        let dist =
                            mapskit::format_distance(place.coordinate.distance_to(&origin));
                        println!("  {} - {} ({})", place.name, place.address.one_line(), dist);
                    }
                    // Remember the results so Route can pick the nearest one.
                    if let Ok(mut slot) = results.lock() {
                        *slot = places.clone();
                    }
                    if let Some(first) = places.first() {
                        map.show_place(first);
                    }
                    for place in &places[1..] {
                        map.add_place(place);
                    }
                }
                Err(e) => eprintln!("[maps_demo] search failed: {e}"),
            }
        });
    }

    fn overlay_hit(&self, x: f64, y: f64) -> bool {
        let (ox, oy, ow, oh) = self.overlay;
        x >= ox as f64 && x <= (ox + ow) as f64 && y >= oy as f64 && y <= (oy + oh) as f64
    }
}

impl App for MapsDemo {
    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
        viewport: Viewport,
        time_secs: f64,
    ) {
        self.watcher.poll(time_secs);
        self.watcher.set_focused(self.focused, time_secs);
        let palette = self.watcher.palette(time_secs);
        self.bg = palette.bg;
        let theme = self.watcher.theme();
        let dark = theme.mode == ThemeMode::Dark;

        self.search.set_theme(theme.mode, palette.accent, theme.glass);
        self.search.set_focused(self.focused);
        self.toolbar.set_theme(theme.mode, theme.glass);
        self.toolbar.set_focused(self.focused);

        self.bar.set_palette(
            palette.titlebar_bg,
            palette.titlebar_text,
            palette.divider,
        );
        self.bar.set_rect(viewport.x, viewport.y, viewport.width);
        self.bar.draw(scene, fonts);

        // The map fills the whole content area; the search + toolbar float
        // over it as glass.
        let top = viewport.y + 31.0;
        let pad = 12.0;
        self.map.place(
            fonts,
            viewport.x,
            top,
            viewport.width,
            (viewport.y + viewport.height - top).max(100.0),
        );
        self.map.draw(scene, fonts, images);

        let gap = 8.0;
        let row_y = top + pad;
        let (bar_w, bar_h) = self.toolbar.measure(fonts);
        let bar_x = viewport.x + viewport.width - pad - bar_w;
        let content_w = (bar_x - gap - (viewport.x + pad)).max(120.0);
        self.search.place(fonts, viewport.x + pad, row_y, content_w, 36.0);
        self.toolbar.place(fonts, bar_x, row_y, bar_w, bar_h);

        self.search.draw(scene, fonts, images);
        self.toolbar.draw(scene, fonts, images);

        let (_, _, _, sh) = self.search.rect();
        let overlay_h = sh.max(bar_h) + 16.0;
        self.overlay = (viewport.x, row_y - 8.0, viewport.width, overlay_h);
        let _ = dark;
    }

    fn background(&self) -> Color {
        self.bg
    }

    /// Glass blur pass so the floating toolbar refracts the map.
    fn wants_backdrop(&self) -> bool {
        true
    }

    fn drag_region(&self) -> Option<(f32, f32, f32, f32)> {
        Some(self.bar.drag_rect())
    }

    fn poll_window_command(&mut self) -> Option<WindowCommand> {
        self.command.take()
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        match self.bar.press(x as f32, y as f32) {
            Some(TrafficAction::Close) => self.command = Some(WindowCommand::Close),
            Some(TrafficAction::Minimize) => self.command = Some(WindowCommand::Minimize),
            Some(TrafficAction::Maximize) => self.command = Some(WindowCommand::ToggleMaximize),
            None => {}
        }
        self.search.mouse_down(x, y);
        self.toolbar.mouse_down(x, y);
        // Presses on the floating glass must not start a map drag.
        if !self.overlay_hit(x, y) {
            self.map.mouse_down(x, y);
        }
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.toolbar.mouse_up(x, y);
        self.map.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.bar.set_hover(x as f32, y as f32);
        self.search.set_hover(x as f32, y as f32);
        self.toolbar.set_hover(x as f32, y as f32);
        self.map.set_hover(x as f32, y as f32);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        let (rx, ry, rw, rh) = self.map.rect();
        let (cx, cy) = self.cursor;
        if cx >= rx as f64 && cx <= (rx + rw) as f64 && cy >= ry as f64 && cy <= (ry + rh) as f64 {
            self.map.mouse_wheel(dx, dy);
        }
    }

    fn text(&mut self, text: &str) {
        self.search.type_text(text);
    }

    fn key(&mut self, key: tontooui::renderer::window::Key) -> () {
        if key == tontooui::renderer::window::Key::Enter && self.search.is_selected() {
            self.run_search();
            return;
        }
        self.search.key(key);
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.bar.set_focused(focused);
    }
}

fn main() {
    if let Err(err) = run("TontooMapsKit", 1000, 700, MapsDemo::new()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
