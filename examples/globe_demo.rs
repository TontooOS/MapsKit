//! Interactive globe demo (TontooUI).
//!
//! Shows the `GlobeView` with markers for four cities. Drag rotates the
//! earth, scroll zooms, the idle spin can be paused and the camera flies
//! between cities.
//!
//! Run with: `cargo run --example globe_demo`

use mapskit::prelude::*;
use tontooui::elements::{Button, Titlebar, TrafficAction, View};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;

const CITIES: &[(&str, f64, f64)] = &[
    ("Berlin", 52.52, 13.405),
    ("Tokyo", 35.68, 139.69),
    ("New York", 40.71, -74.01),
    ("Sydney", -33.87, 151.21),
];

struct GlobeDemo {
    bar: Titlebar,
    city_buttons: Vec<Button>,
    spin_button: Button,
    zoom_in: Button,
    zoom_out: Button,
    globe: GlobeView,
    watcher: ThemeWatcher,
    focused: bool,
    bg: Color,
    command: Option<WindowCommand>,
    cursor: (f64, f64),
}

impl GlobeDemo {
    fn new() -> Self {
        let config = MapsConfiguration::new().style(MapStyle::Dark);
        let globe = GlobeView::new(&config);
        globe.set_center(Coordinate::new(52.52, 13.405));
        globe.set_distance(3.0);
        globe.set_auto_rotate(true);
        globe.set_markers(
            CITIES
                .iter()
                .map(|(name, lat, lon)| GlobeMarker::new(Coordinate::new(*lat, *lon), *name))
                .collect(),
        );

        let mut city_buttons = Vec::new();
        for (name, lat, lon) in CITIES {
            let globe = globe.clone();
            let name = *name;
            let (lat, lon) = (*lat, *lon);
            city_buttons.push(Button::new(name).on_press(move || {
                globe.set_center(Coordinate::new(lat, lon));
                globe.set_distance(3.0);
                println!("[globe_demo] flying to {name}");
            }));
        }

        let spin_globe = globe.clone();
        let spin_button = Button::new("Pause").on_press(move || {
            let enabled = !spin_globe.auto_rotate();
            spin_globe.set_auto_rotate(enabled);
            println!(
                "[globe_demo] auto-rotate {}",
                if enabled { "on" } else { "off" }
            );
        });

        let zoom_globe = globe.clone();
        let zoom_in = Button::new("Zoom +").on_press(move || {
            zoom_globe.set_distance((zoom_globe.distance() - 0.4).max(1.15));
            println!("[globe_demo] distance {:.2}", zoom_globe.distance());
        });

        let zoom_globe = globe.clone();
        let zoom_out = Button::new("Zoom -").on_press(move || {
            zoom_globe.set_distance((zoom_globe.distance() + 0.5).min(6.0));
        });

        println!(
            "[globe_demo] drag rotates, scroll zooms, {} markers shown",
            CITIES.len()
        );

        Self {
            bar: Titlebar::new("TontooMapsKit Globe"),
            city_buttons,
            spin_button,
            zoom_in,
            zoom_out,
            globe,
            watcher: ThemeWatcher::new(),
            focused: true,
            bg: tontooui::renderer::window::BACKGROUND,
            command: None,
            cursor: (0.0, 0.0),
        }
    }

}

impl App for GlobeDemo {
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
        let _ = self.watcher.theme().mode == ThemeMode::Dark;

        self.bar.set_palette(
            palette.titlebar_bg,
            palette.titlebar_text,
            palette.divider,
        );
        self.bar.set_rect(viewport.x, viewport.y, viewport.width);
        self.bar.draw(scene, fonts);

        let top = viewport.y + 31.0;
        let pad = 12.0;
        let gap = 8.0;
        let row_h = 36.0;
        let x0 = viewport.x + pad;
        let mut x = x0;
        let content_w = viewport.width - pad * 2.0;

        // City buttons share the row with the control buttons.
        let city_w = ((content_w - gap * 7.0 - 3.0 * 80.0) / 4.0).max(70.0);
        for button in &mut self.city_buttons {
            button.place(fonts, x, top + pad, city_w, row_h);
            button.draw(scene, fonts, images);
            x += city_w + gap;
        }
        for button in [&mut self.spin_button, &mut self.zoom_in, &mut self.zoom_out] {
            button.place(fonts, x, top + pad, 80.0, row_h);
            button.draw(scene, fonts, images);
            x += 80.0 + gap;
        }

        let globe_y = top + pad + row_h + pad;
        let globe_h = (viewport.y + viewport.height - globe_y - pad).max(100.0);
        self.globe.place(fonts, x0, globe_y, content_w, globe_h);
        self.globe.draw(scene, fonts, images);
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
        for button in &mut self.city_buttons {
            button.mouse_down(x, y);
        }
        self.spin_button.mouse_down(x, y);
        self.zoom_in.mouse_down(x, y);
        self.zoom_out.mouse_down(x, y);
        self.globe.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        for button in &mut self.city_buttons {
            button.mouse_up(x, y);
        }
        self.spin_button.mouse_up(x, y);
        self.zoom_in.mouse_up(x, y);
        self.zoom_out.mouse_up(x, y);
        self.globe.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.bar.set_hover(x as f32, y as f32);
        for button in &mut self.city_buttons {
            button.set_hover(x as f32, y as f32);
        }
        self.spin_button.set_hover(x as f32, y as f32);
        self.zoom_in.set_hover(x as f32, y as f32);
        self.zoom_out.set_hover(x as f32, y as f32);
        self.globe.set_hover(x as f32, y as f32);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        let (rx, ry, rw, rh) = self.globe.rect();
        let (cx, cy) = self.cursor;
        if cx >= rx as f64 && cx <= (rx + rw) as f64 && cy >= ry as f64 && cy <= (ry + rh) as f64 {
            self.globe.mouse_wheel(dx, dy);
        }
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.bar.set_focused(focused);
    }
}

fn main() {
    if let Err(err) = run("TontooMapsKit Globe", 960, 700, GlobeDemo::new()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
