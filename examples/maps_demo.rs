//! Interactive TontooMapsKit demo (TontooUI).
//!
//! A map with a search field, a route button and a user location button.
//! Search and routing run on worker threads so the UI stays responsive;
//! results write straight into the shared map model.
//!
//! Run with: `cargo run --example maps_demo`
//!
//! Controls: click the map and drag to pan, scroll to zoom, click the
//! Satellite/Standard pill to switch layers. Type a query and press Enter
//! to search from your location (nearest first), then press Route for
//! driving directions from your location to the nearest result.

use mapskit::prelude::*;
use std::sync::{Arc, Mutex};
use tontooui::elements::{Button, SearchField, Titlebar, TrafficAction, View};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;

/// Latest search results, shared with the button callbacks.
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

struct MapsDemo {
    bar: Titlebar,
    search: SearchField,
    route_button: Button,
    locate_button: Button,
    map: MapView,
    results: SharedResults,
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

        // Route: from your location to the nearest search result. The
        // cloned map and results share their models, so the worker thread
        // updates the UI directly and the shell picks it up on the next
        // frame.
        let route_map = map.clone();
        let route_results: SharedResults = Arc::new(Mutex::new(Vec::new()));
        let route_button = Button::new("Route").on_press({
            let map = route_map.clone();
            let results = route_results.clone();
            move || {
                let map = map.clone();
                let results = results.clone();
                std::thread::spawn(move || {
                    let places = results.lock().map(|r| r.clone()).unwrap_or_default();
                    if places.is_empty() {
                        println!("[maps_demo] search for a place first (type + Enter)");
                        return;
                    }
                    // Origin: user location first, so "Aldi" means the Aldi
                    // near you, not near the map center.
                    let (origin, from_center) = locate_origin(&map);
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
                    match ProviderChain::default_providers()
                        .route(origin, dest.coordinate, TravelMode::Driving)
                    {
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
                });
            }
        });

        let locate_map = map.clone();
        let locate_button = Button::new("Locate").on_press(move || {
            locate_map.show_user_location();
        });

        Self {
            bar: Titlebar::new("TontooMapsKit"),
            search,
            route_button,
            locate_button,
            map,
            results: route_results,
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
                        let dist = mapskit::format_distance(
                            place.coordinate.distance_to(&origin),
                        );
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
        let dark = self.watcher.theme().mode == ThemeMode::Dark;

        self.bar.set_palette(
            palette.titlebar_bg,
            palette.titlebar_text,
            palette.divider,
        );
        self.bar.set_rect(viewport.x, viewport.y, viewport.width);
        self.bar.draw(scene, fonts);

        let top = viewport.y + 31.0;
        let pad = 12.0;
        let row_h = 36.0;
        let btn_w = 90.0;
        let gap = 8.0;
        let content_w = viewport.width - pad * 2.0;
        let search_w = (content_w - btn_w * 2.0 - gap * 2.0).max(120.0);
        let x0 = viewport.x + pad;

        self.search.place(fonts, x0, top + pad, search_w, row_h);
        self.route_button.place(fonts, x0 + search_w + gap, top + pad, btn_w, row_h);
        self.locate_button.place(
            fonts,
            x0 + search_w + gap + btn_w + gap,
            top + pad,
            btn_w,
            row_h,
        );

        let map_y = top + pad + row_h + pad;
        let map_h = (viewport.y + viewport.height - map_y - pad).max(100.0);
        self.map.place(fonts, x0, map_y, content_w, map_h);

        self.search.draw(scene, fonts, images);
        self.route_button.draw(scene, fonts, images);
        self.locate_button.draw(scene, fonts, images);
        self.map.draw(scene, fonts, images);

        let _ = dark;
    }

    fn background(&self) -> Color {
        self.bg
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
        self.route_button.mouse_down(x, y);
        self.locate_button.mouse_down(x, y);
        self.map.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.route_button.mouse_up(x, y);
        self.locate_button.mouse_up(x, y);
        self.map.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.bar.set_hover(x as f32, y as f32);
        self.search.set_hover(x as f32, y as f32);
        self.route_button.set_hover(x as f32, y as f32);
        self.locate_button.set_hover(x as f32, y as f32);
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
