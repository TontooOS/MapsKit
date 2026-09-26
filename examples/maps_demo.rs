//! Interactive TontooMapsKit demo (TontooUI).
//!
//! A map with a search field, a route demo button and a user location
//! button. Search and routing run on worker threads so the UI stays
//! responsive; results write straight into the shared map model.
//!
//! Run with: `cargo run --example maps_demo`
//!
//! Controls: click the map and drag to pan, scroll to zoom, click the
//! Satellite/Standard pill to switch layers. Type a query and press Enter
//! to search.

use mapskit::prelude::*;
use tontooui::elements::{Button, SearchField, Titlebar, TrafficAction, View};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;

struct MapsDemo {
    bar: Titlebar,
    search: SearchField,
    route_button: Button,
    locate_button: Button,
    map: MapView,
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

        // Route demo: Brandenburg Gate -> Alexanderplatz. The cloned map
        // shares the same model, so the worker thread updates the UI
        // directly and the shell picks it up on the next frame.
        let route_map = map.clone();
        let route_button = Button::new("Route").on_press(move || {
            let map = route_map.clone();
            std::thread::spawn(move || {
                let from = Coordinate::new(52.5163, 13.3777);
                let to = Coordinate::new(52.5219, 13.4132);
                match ProviderChain::default_providers().route(from, to, TravelMode::Driving) {
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
        let center = self.map.camera().center;
        let map = self.map.clone();
        std::thread::spawn(move || {
            match ProviderChain::default_providers().search(&query, Some(center), 10) {
                Ok(places) => {
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
                    for place in &places {
                        println!("  {} - {}", place.name, place.address.one_line());
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
