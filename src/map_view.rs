//! `MapView` — the interactive 2D map view, following Apple's `MKMapView`.
//!
//! The view renders slippy map tiles with Cairo, supports pan (drag), zoom
//! (scroll wheel, double click) and draws annotations, polylines, polygons
//! and circles. Tiles are fetched on worker threads and handed back to the
//! GTK main loop through a channel, so the UI never blocks.
//!
//! ```rust,no_run
//! use mapskit::prelude::*;
//!
//! let map = MapView::new(&MapsConfiguration::new().style(MapStyle::Light));
//! map.set_center(Coordinate::new(52.52, 13.405), 12.0);
//! map.add_annotation(Annotation::new(Coordinate::new(52.52, 13.405), "Berlin"));
//!
//! // map.widget() can be embedded in any GTK4 container or UIKit view.
//! ```

use crate::camera::MapCamera;
use crate::config::{MapStyle, MapsConfiguration};
use crate::overlays::{Annotation, CircleOverlay, Overlay, Polyline};
use crate::providers::ProviderChain;
use crate::tiles::{self, TileCache, TileKey};
use crate::types::{Coordinate, MapRegion, Route};

use gdk_pixbuf::Pixbuf;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::prelude::*;
use gtk::{DrawingArea, EventControllerScroll, GestureClick, GestureDrag};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const TILE_SIZE: f64 = 256.0;
const ACCENT: (f64, f64, f64) = (1.0, 0.42, 0.17); // TontooOS orange #FF6B2B

/// Background colors per style (TontooOS dark/light surfaces).
fn background_for_style(style: MapStyle) -> (f64, f64, f64) {
    match style {
        MapStyle::Dark => (0x1d as f64 / 255.0, 0x1d as f64 / 255.0, 0x1d as f64 / 255.0),
        _ => (0xec as f64 / 255.0, 0xec as f64 / 255.0, 0xec as f64 / 255.0),
    }
}

fn log_debug(message: &str) {
    if std::env::var("MAPSKIT_DEBUG").as_deref() == Ok("1") {
        eprintln!("[mapskit] {message}");
    }
}

struct MapState {
    config: MapsConfiguration,
    chain: Arc<ProviderChain>,
    camera: MapCamera,
    /// The currently rendered base map style (switchable at runtime).
    style: MapStyle,
    tiles: HashMap<TileKey, Option<Pixbuf>>,
    pending: HashSet<TileKey>,
    annotations: Vec<Annotation>,
    overlays: Vec<Overlay>,
    dragging: Option<(f64, f64)>,
    drag_moved: bool,
    /// Last processed cumulative drag offset, so `drag_update` deltas are
    /// applied exactly once (GTK reports offsets from the drag start).
    last_drag_offset: Option<(f64, f64)>,
    /// Screen rect of the layer toggle pill, refreshed on every frame.
    toggle_rect: std::cell::Cell<Option<(f64, f64, f64, f64)>>,
    /// Whether the active press started on the layer toggle.
    toggle_active: bool,
    width: f64,
    height: f64,
    offline: bool,
    /// The user's location, when shown via `show_user_location`.
    user_location: Option<Coordinate>,
    on_annotation_tapped: Option<Box<dyn Fn(&Annotation)>>,
    on_camera_changed: Option<Box<dyn Fn(&MapCamera)>>,
}

impl MapState {
    /// The provider serving base map tiles for the active style.
    fn tile_provider(&self) -> Arc<dyn crate::providers::MapProvider> {
        self.chain.tile_provider_for(self.style).cloned().unwrap_or_else(|| {
            self.chain
                .tile_provider_for(MapStyle::Standard)
                .cloned()
                .expect("chain empty")
        })
    }

    fn attribution_key(&self) -> &'static str {
        let name = self.tile_provider().name();
        if name.contains("CARTO") {
            "mapskit.map.attribution.carto"
        } else if name.contains("Esri") {
            "mapskit.map.attribution.esri"
        } else {
            "mapskit.map.attribution.osm"
        }
    }

    /// Fractional world pixel position of a coordinate at the integer zoom.
    fn world_px(&self, coordinate: Coordinate) -> (f64, f64) {
        let z = self.camera.zoom.round() as u8;
        let (fx, fy) = tiles::tile_for(coordinate, z);
        (fx * TILE_SIZE, fy * TILE_SIZE)
    }

    /// Screen position of a coordinate in the current viewport.
    fn coord_to_screen(&self, coordinate: Coordinate) -> (f64, f64) {
        let z = self.camera.zoom.round() as u8;
        let scale = (self.camera.zoom - f64::from(z)).exp2();
        let (wx, wy) = self.world_px(coordinate);
        let (cx, cy) = self.world_px(self.camera.center);
        let origin_x = cx - self.width / 2.0;
        let origin_y = cy - self.height / 2.0;
        ((wx - origin_x) * scale, (wy - origin_y) * scale)
    }

    /// The coordinate under a screen point.
    fn screen_to_coord(&self, x: f64, y: f64) -> Coordinate {
        let z = self.camera.zoom.round() as u8;
        let scale = (self.camera.zoom - f64::from(z)).exp2();
        let (cx, cy) = self.world_px(self.camera.center);
        let origin_x = cx - self.width / 2.0;
        let origin_y = cy - self.height / 2.0;
        let wx = x / scale + origin_x;
        let wy = y / scale + origin_y;
        let n = f64::from(1u32 << z);
        let lon = wx / TILE_SIZE / n * 360.0 - 180.0;
        let lat = ((std::f64::consts::PI * (1.0 - 2.0 * wy / TILE_SIZE / n)).sinh()).atan().to_degrees();
        Coordinate::new(lat.clamp(-85.051_128_78, 85.051_128_78), lon)
    }

    /// Zooms around a screen anchor so the coordinate under it stays put.
    fn zoom_around(&mut self, x: f64, y: f64, delta: f64) {
        let anchor = self.screen_to_coord(x, y);
        let new_zoom = (self.camera.zoom + delta).clamp(0.0, f64::from(tiles::MAX_ZOOM));
        if (new_zoom - self.camera.zoom).abs() < f64::EPSILON {
            return;
        }
        self.camera.zoom = new_zoom;

        // Shift the center so the anchor stays under (x, y).
        let z = self.camera.zoom.round() as u8;
        let scale = (self.camera.zoom - f64::from(z)).exp2();
        let (fx, fy) = tiles::tile_for(anchor, z);
        let (ax, ay) = (fx * TILE_SIZE, fy * TILE_SIZE);
        let origin_x = ax - x / scale;
        let origin_y = ay - y / scale;
        let center_wx = origin_x + self.width / 2.0;
        let center_wy = origin_y + self.height / 2.0;
        let n = f64::from(1u32 << z);
        let lon = center_wx / TILE_SIZE / n * 360.0 - 180.0;
        let lat =
            ((std::f64::consts::PI * (1.0 - 2.0 * center_wy / TILE_SIZE / n)).sinh()).atan().to_degrees();
        self.camera.center = Coordinate::new(
            lat.clamp(-85.051_128_78, 85.051_128_78),
            lon.clamp(-180.0, 180.0),
        );
    }

    /// Zoom level at which the bounding box fits into the viewport.
    fn fitting_zoom(&self, bbox: &crate::types::BoundingBox) -> f64 {
        for z in (0..=tiles::MAX_ZOOM).rev() {
            let (min_fx, min_fy) = tiles::tile_for(Coordinate::new(bbox.south, bbox.west), z);
            let (max_fx, max_fy) = tiles::tile_for(Coordinate::new(bbox.north, bbox.east), z);
            let w = (max_fx - min_fx).abs() * TILE_SIZE;
            let h = (max_fy - min_fy).abs() * TILE_SIZE;
            if w <= self.width && h <= self.height {
                return f64::from(z);
            }
        }
        0.0
    }

    /// Annotation hit test in screen space (topmost first).
    fn hit_annotation(&self, x: f64, y: f64) -> Option<usize> {
        const RADIUS: f64 = 18.0;
        for (index, annotation) in self.annotations.iter().enumerate().rev() {
            let (sx, sy) = self.coord_to_screen(annotation.coordinate);
            if (x - sx).hypot(y - sy) <= RADIUS {
                return Some(index);
            }
        }
        None
    }

    /// Whether the point is inside the layer toggle pill.
    fn toggle_hit(&self, x: f64, y: f64) -> bool {
        match self.toggle_rect.get() {
            Some((rx, ry, rw, rh)) => x >= rx && x <= rx + rw && y >= ry && y <= ry + rh,
            None => false,
        }
    }

    /// Switches the base layer and drops all cached tiles of the old one.
    fn set_base_style(&mut self, style: MapStyle) {
        if self.style == style {
            return;
        }
        self.style = style;
        self.tiles.clear();
        self.pending.clear();
        log_debug(&format!("base layer switched to {style:?}"));
    }
}

/// Requests all visible tiles that are not loaded yet.
fn request_visible_tiles(state: &Rc<RefCell<MapState>>, area: &DrawingArea) {
    let (wanted, provider_name, cache_source, user_agent, timeout, cache_config) = {
        let s = state.borrow();
        let zoom = s.camera.zoom.round() as u8;
        let scale = (s.camera.zoom - f64::from(zoom)).exp2();

        let view_tiles_x = s.width / (TILE_SIZE * scale);
        let view_tiles_y = s.height / (TILE_SIZE * scale);

        let (cfx, cfy) = tiles::tile_for(s.camera.center, zoom);
        let max_tiles = (1i64 << zoom) - 1;
        let min_x = ((cfx - view_tiles_x / 2.0).floor() as i64).clamp(0, max_tiles);
        let max_x = ((cfx + view_tiles_x / 2.0).floor() as i64).clamp(0, max_tiles);
        let min_y = (cfy - view_tiles_y / 2.0).floor().max(0.0) as i64;
        let max_y = ((cfy + view_tiles_y / 2.0).floor() as i64).min(max_tiles);

        let mut list = Vec::new();
        for ty in min_y..=max_y {
            for tx in min_x..=max_x {
                let key = TileKey::new(zoom, tx as u32, ty as u32);
                if !s.tiles.contains_key(&key) && !s.pending.contains(&key) {
                    list.push(key);
                }
            }
        }
        let provider = s.tile_provider();
        let cache_source = tiles::cache_source_key(provider.as_ref());
        (
            list,
            provider.name().to_string(),
            cache_source,
            s.config.user_agent.clone(),
            s.config.timeout_seconds,
            s.config.clone(),
        )
    };

    if wanted.is_empty() {
        return;
    }

    let (sender, receiver) = async_channel::unbounded::<(TileKey, Result<Vec<u8>, String>)>();

    for key in wanted {
        let inserted = {
            let mut s = state.borrow_mut();
            if s.pending.contains(&key) || s.tiles.contains_key(&key) {
                false
            } else {
                s.pending.insert(key);
                true
            }
        };
        if !inserted {
            continue;
        }

        let provider = {
            let s = state.borrow();
            s.chain.tile_provider_for(s.style).cloned().expect("chain empty")
        };

        let sender = sender.clone();
        let provider_name = provider_name.clone();
        let cache_source = cache_source.clone();
        let user_agent = user_agent.clone();
        let cache_config = cache_config.clone();
        std::thread::spawn(move || {
            let cache = TileCache::new(&cache_config, &provider_name, &cache_source);
            let result =
                tiles::fetch_tile(provider.as_ref(), &cache, key, &user_agent, timeout)
                    .map_err(|e| e.to_string());
            let _ = sender.send_blocking((key, result));
        });
    }
    drop(sender);

    let state_rc = state.clone();
    let area = area.clone();
    glib::spawn_future_local(async move {
        while let Ok((key, result)) = receiver.recv().await {
            {
                let mut s = state_rc.borrow_mut();
                s.pending.remove(&key);
                match result {
                    Ok(bytes) => {
                        s.offline = false;
                        let loader = gdk_pixbuf::PixbufLoader::new();
                        let decoded = loader
                            .write(&bytes)
                            .is_ok()
                            .then(|| loader.close().ok())
                            .flatten()
                            .and_then(|_| loader.pixbuf());
                        s.tiles.insert(key, decoded);
                    }
                    Err(e) => {
                        s.offline = true;
                        s.tiles.insert(key, None);
                        log_debug(&format!(
                            "tile {}/{}/{} failed: {}",
                            key.z, key.x, key.y, e
                        ));
                    }
                }
            }
            area.queue_draw();
        }
    });
}

// ═══════════════════════════════════════════════════════════════
// Rendering
// ═══════════════════════════════════════════════════════════════

fn render(state: &MapState, ctx: &gtk::cairo::Context) {
    let (bg_r, bg_g, bg_b) = background_for_style(state.style);
    ctx.set_source_rgb(bg_r, bg_g, bg_b);
    ctx.paint().ok();

    draw_tiles(state, ctx);
    draw_overlays(state, ctx);
    draw_user_location(state, ctx);
    draw_annotations(state, ctx);
    draw_badges(state, ctx);
    draw_layer_toggle(state, ctx);
}

/// The blue pulsing-dot style user location marker.
fn draw_user_location(state: &MapState, ctx: &gtk::cairo::Context) {
    let Some(coordinate) = state.user_location else {
        return;
    };
    let (sx, sy) = state.coord_to_screen(coordinate);

    // Accuracy halo.
    let z = state.camera.zoom.round() as u8;
    let mpp = tiles::meters_per_pixel(z, coordinate.latitude);
    let scale = (state.camera.zoom - f64::from(z)).exp2();
    let halo_px = 50.0 / mpp.max(0.5) * scale;
    ctx.set_source_rgba(0.10, 0.45, 0.95, 0.15);
    ctx.new_path();
    ctx.arc(sx, sy, halo_px.clamp(12.0, 200.0), 0.0, std::f64::consts::TAU);
    ctx.fill().ok();

    // White ring + blue core.
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.new_path();
    ctx.arc(sx, sy, 7.0, 0.0, std::f64::consts::TAU);
    ctx.fill().ok();
    ctx.set_source_rgb(0.10, 0.50, 1.0);
    ctx.new_path();
    ctx.arc(sx, sy, 5.5, 0.0, std::f64::consts::TAU);
    ctx.fill().ok();
}

fn draw_tiles(state: &MapState, ctx: &gtk::cairo::Context) {
    let z = state.camera.zoom.round() as u8;
    let scale = (state.camera.zoom - f64::from(z)).exp2();
    let tile_draw_size = TILE_SIZE * scale;

    let (cx, cy) = state.world_px(state.camera.center);
    let origin_x = cx - state.width / 2.0;
    let origin_y = cy - state.height / 2.0;

    let max_tiles = (1u32 << z) as i64;
    let min_tx = (origin_x / TILE_SIZE).floor().max(0.0) as i64;
    let max_tx = (((origin_x + state.width) / TILE_SIZE).floor() as i64).min(max_tiles - 1);
    let min_ty = (origin_y / TILE_SIZE).floor().max(0.0) as i64;
    let max_ty = (((origin_y + state.height) / TILE_SIZE).floor() as i64).min(max_tiles - 1);

    for ty in min_ty..=max_ty {
        for tx in min_tx..=max_tx {
            let key = TileKey::new(z, tx as u32, ty as u32);
            let screen_x = tx as f64 * TILE_SIZE - origin_x;
            let screen_y = ty as f64 * TILE_SIZE - origin_y;
            match state.tiles.get(&key) {
                Some(Some(pixbuf)) => {
                    ctx.save().ok();
                    ctx.scale(scale, scale);
                    ctx.set_source_pixbuf(pixbuf, screen_x / scale, screen_y / scale);
                    ctx.paint().ok();
                    ctx.restore().ok();
                }
                _ => {
                    // Placeholder while loading.
                    let (r, g, b, a) = match state.style {
                        MapStyle::Dark => (1.0, 1.0, 1.0, 0.05),
                        _ => (0.0, 0.0, 0.0, 0.06),
                    };
                    ctx.set_source_rgba(r, g, b, a);
                    ctx.rectangle(screen_x, screen_y, tile_draw_size, tile_draw_size);
                    ctx.fill().ok();
                }
            }
        }
    }
}

fn parse_css_color(color: &str, fallback: (f64, f64, f64)) -> (f64, f64, f64, f64) {
    let color = color.trim();
    if let Some(hex) = color.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return (
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                    1.0,
                );
            }
        }
    }
    if let Some(rest) = color.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f64> = rest.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if parts.len() == 4 {
            return (parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, parts[3]);
        }
    }
    (fallback.0, fallback.1, fallback.2, 1.0)
}

fn draw_overlays(state: &MapState, ctx: &gtk::cairo::Context) {
    for overlay in &state.overlays {
        match overlay {
            Overlay::Polyline(line) => {
                if line.points.len() < 2 {
                    continue;
                }
                let (r, g, b, a) = parse_css_color(&line.color, ACCENT);
                ctx.set_source_rgba(r, g, b, a);
                ctx.set_line_width(line.width);
                ctx.set_line_cap(gtk::cairo::LineCap::Round);
                ctx.set_line_join(gtk::cairo::LineJoin::Round);
                let mut first = true;
                for point in &line.points {
                    let (sx, sy) = state.coord_to_screen(*point);
                    if first {
                        ctx.move_to(sx, sy);
                        first = false;
                    } else {
                        ctx.line_to(sx, sy);
                    }
                }
                ctx.stroke().ok();
            }
            Overlay::Polygon(polygon) => {
                if polygon.points.len() < 3 {
                    continue;
                }
                ctx.save().ok();
                let mut first = true;
                for point in &polygon.points {
                    let (sx, sy) = state.coord_to_screen(*point);
                    if first {
                        ctx.move_to(sx, sy);
                        first = false;
                    } else {
                        ctx.line_to(sx, sy);
                    }
                }
                ctx.close_path();
                if !polygon.fill_color.is_empty() {
                    let (r, g, b, a) = parse_css_color(&polygon.fill_color, ACCENT);
                    ctx.set_source_rgba(r, g, b, a);
                    ctx.fill_preserve().ok();
                }
                if !polygon.stroke_color.is_empty() {
                    let (r, g, b, a) = parse_css_color(&polygon.stroke_color, ACCENT);
                    ctx.set_source_rgba(r, g, b, a);
                    ctx.set_line_width(2.0);
                    ctx.stroke().ok();
                }
                ctx.restore().ok();
            }
            Overlay::Circle(circle) => {
                draw_circle(state, ctx, circle);
            }
        }
    }
}

fn draw_circle(state: &MapState, ctx: &gtk::cairo::Context, circle: &CircleOverlay) {
    // Convert the radius in meters to pixels at the center latitude.
    let z = state.camera.zoom.round() as u8;
    let mpp = tiles::meters_per_pixel(z, circle.center.latitude);
    let scale = (state.camera.zoom - f64::from(z)).exp2();
    let radius_px = circle.radius_m / mpp.max(0.01) * scale;
    let (sx, sy) = state.coord_to_screen(circle.center);

    ctx.save().ok();
    ctx.new_path();
    ctx.arc(sx, sy, radius_px.max(2.0), 0.0, std::f64::consts::TAU);
    if !circle.fill_color.is_empty() {
        let (r, g, b, a) = parse_css_color(&circle.fill_color, (0.0, 0.9, 1.0));
        ctx.set_source_rgba(r, g, b, a);
        ctx.fill_preserve().ok();
    }
    if !circle.stroke_color.is_empty() {
        let (r, g, b, a) = parse_css_color(&circle.stroke_color, (0.0, 0.9, 1.0));
        ctx.set_source_rgba(r, g, b, a);
        ctx.set_line_width(2.0);
        ctx.stroke().ok();
    }
    ctx.restore().ok();
}

fn draw_annotations(state: &MapState, ctx: &gtk::cairo::Context) {
    ctx.select_font_face(
        "SF Pro Text",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    ctx.set_font_size(12.0);

    for annotation in &state.annotations {
        let (sx, sy) = state.coord_to_screen(annotation.coordinate);
        if sx < -40.0 || sy < -60.0 || sx > state.width + 40.0 || sy > state.height + 60.0 {
            continue;
        }

        let accent = if annotation.selected {
            (1.0, 0.55, 0.35)
        } else {
            ACCENT
        };

        // Pin shadow
        ctx.set_source_rgba(0.0, 0.0, 0.0, 0.25);
        ctx.new_path();
        ctx.arc(sx, sy - 14.0, 7.5, 0.0, std::f64::consts::TAU);
        ctx.fill().ok();

        // Pin head
        ctx.set_source_rgb(accent.0, accent.1, accent.2);
        ctx.new_path();
        ctx.arc(sx, sy - 14.0, 7.0, 0.0, std::f64::consts::TAU);
        ctx.fill().ok();

        // Pin tip
        ctx.new_path();
        ctx.move_to(sx - 3.0, sy - 9.0);
        ctx.line_to(sx, sy);
        ctx.line_to(sx + 3.0, sy - 9.0);
        ctx.close_path();
        ctx.fill().ok();

        // Label plate
        let title = &annotation.title;
        let (text_x, text_y, text_w, text_h) = ctx.text_extents(title).map(|e| {
            (sx + 10.0, sy - 20.0, e.width(), e.height())
        }).unwrap_or((sx + 10.0, sy - 20.0, 40.0, 10.0));

        ctx.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        rounded_rect(ctx, text_x - 5.0, text_y - 3.0, text_w + 10.0, text_h + 8.0, 5.0);
        ctx.fill().ok();

        ctx.set_source_rgb(1.0, 1.0, 1.0);
        ctx.move_to(text_x, text_y + text_h);
        ctx.show_text(title).ok();
    }
}

fn rounded_rect(ctx: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    ctx.new_path();
    ctx.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    ctx.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    ctx.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    ctx.close_path();
}

fn draw_badges(state: &MapState, ctx: &gtk::cairo::Context) {
    ctx.select_font_face(
        "SF Pro Text",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    ctx.set_font_size(10.0);

    // Attribution bottom-right.
    let attribution = crate::lang::t_or(state.attribution_key(), "\u{a9} OpenStreetMap contributors");
    if let Ok(extents) = ctx.text_extents(&attribution) {
        let pad = 6.0;
        let w = extents.width() + pad * 2.0;
        let h = extents.height() + pad;
        let x = state.width - w - 4.0;
        let y = state.height - h - 4.0;
        ctx.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        rounded_rect(ctx, x, y, w, h, 4.0);
        ctx.fill().ok();
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        ctx.move_to(x + pad, y + pad + extents.height());
        ctx.show_text(&attribution).ok();
    }

    // Offline badge top-left.
    if state.offline {
        let text = crate::lang::t_or("mapskit.map.offline", "Offline");
        if let Ok(extents) = ctx.text_extents(&text) {
            let pad = 6.0;
            let w = extents.width() + pad * 2.0;
            let h = extents.height() + pad;
            ctx.set_source_rgba(0.55, 0.15, 0.05, 0.85);
            rounded_rect(ctx, 6.0, 6.0, w, h, 4.0);
            ctx.fill().ok();
            ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            ctx.move_to(6.0 + pad, 6.0 + pad + extents.height());
            ctx.show_text(&text).ok();
        }
    }
}

/// The pill button label: shows the layer the tap switches to.
fn layer_toggle_label(state: &MapState) -> String {
    if state.style == MapStyle::Satellite {
        crate::lang::t_or("mapskit.map.standard", "Standard")
    } else {
        crate::lang::t_or("mapskit.map.satellite", "Satellite")
    }
}

/// Draws the layer toggle pill at the bottom left.
fn draw_layer_toggle(state: &MapState, ctx: &gtk::cairo::Context) {
    ctx.select_font_face(
        "SF Pro Text",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    ctx.set_font_size(11.0);

    let label = layer_toggle_label(state);
    let Ok(extents) = ctx.text_extents(&label) else {
        return;
    };

    const PAD_X: f64 = 12.0;
    const PAD_Y: f64 = 7.0;
    const MARGIN: f64 = 10.0;
    let w = extents.width() + PAD_X * 2.0;
    let h = extents.height() + PAD_Y * 2.0;
    let x = MARGIN;
    let y = state.height - h - MARGIN;

    // Frosted glass pill with hairline border.
    let dark = matches!(state.style, MapStyle::Dark | MapStyle::Satellite);
    let (fill, line, text) = if dark {
        (0.12_f64, 1.0_f64, (1.0, 1.0, 1.0))
    } else {
        (0.85, 0.0, (0.12, 0.12, 0.13))
    };
    ctx.set_source_rgba(0.0, 0.0, 0.0, fill);
    rounded_rect(ctx, x, y, w, h, h / 2.0);
    ctx.fill().ok();
    ctx.set_source_rgba(line, line, line, 0.18);
    rounded_rect(ctx, x + 0.5, y + 0.5, w - 1.0, h - 1.0, h / 2.0);
    ctx.set_line_width(1.0);
    ctx.stroke().ok();

    ctx.set_source_rgb(text.0, text.1, text.2);
    ctx.move_to(x + PAD_X, y + PAD_Y + extents.height());
    ctx.show_text(&label).ok();

    // Publish the rect for hit testing.
    state.toggle_rect.set(Some((x, y, w, h)));
}

// ═══════════════════════════════════════════════════════════════
// MapView
// ═══════════════════════════════════════════════════════════════

/// An interactive 2D map view.
pub struct MapView {
    area: DrawingArea,
    state: Rc<RefCell<MapState>>,
}

impl MapView {
    /// Creates a map view from the configuration.
    pub fn new(config: &MapsConfiguration) -> Self {
        let chain = Arc::new(ProviderChain::with_config(config));
        let state = Rc::new(RefCell::new(MapState {
            config: config.clone(),
            chain,
            camera: MapCamera::default(),
            style: config.style,
            tiles: HashMap::new(),
            pending: HashSet::new(),
            annotations: Vec::new(),
            overlays: Vec::new(),
            dragging: None,
            drag_moved: false,
            last_drag_offset: None,
            toggle_rect: std::cell::Cell::new(None),
            toggle_active: false,
            width: 400.0,
            height: 300.0,
            offline: false,
            user_location: None,
            on_annotation_tapped: None,
            on_camera_changed: None,
        }));

        let area = DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);

        let view = Self { area: area.clone(), state };

        view.setup_render(&area);
        view.setup_gestures(&area);
        view.setup_resize(&area);

        view
    }

    /// The underlying GTK widget for embedding.
    pub fn widget(&self) -> &DrawingArea {
        &self.area
    }

    /// Switches the base layer to satellite imagery (or back to the
    /// configured base style). Cached tiles of the old layer are dropped.
    pub fn set_satellite(&self, enabled: bool) {
        {
            let mut s = self.state.borrow_mut();
            let style = if enabled {
                MapStyle::Satellite
            } else {
                s.config.style
            };
            s.set_base_style(style);
        }
        self.area.queue_draw();
    }

    /// Whether satellite imagery is currently shown.
    pub fn is_satellite(&self) -> bool {
        self.state.borrow().style == MapStyle::Satellite
    }

    fn setup_render(&self, area: &DrawingArea) {
        let state = self.state.clone();
        area.set_draw_func(move |_area, ctx, width, height| {
            {
                let mut s = state.borrow_mut();
                s.width = f64::from(width);
                s.height = f64::from(height);
            }
            request_visible_tiles(&state, _area);
            let s = state.borrow();
            render(&s, ctx);
        });
    }

    fn setup_resize(&self, area: &DrawingArea) {
        let state = self.state.clone();
        area.connect_resize(move |_, width, height| {
            let mut s = state.borrow_mut();
            s.width = f64::from(width);
            s.height = f64::from(height);
        });
    }

    fn setup_gestures(&self, area: &DrawingArea) {
        // Tap detection (annotation hits) via GestureClick.
        let click = GestureClick::new();
        click.set_button(0);
        let state_click = self.state.clone();
        let area_click = area.clone();
        click.connect_pressed(move |_click, _n, x, y| {
            let mut s = state_click.borrow_mut();
            if s.toggle_hit(x, y) {
                // Layer toggle press: switch immediately and swallow the
                // sequence so no pan or annotation tap starts.
                let next = if s.style == MapStyle::Satellite {
                    s.config.style
                } else {
                    MapStyle::Satellite
                };
                s.set_base_style(next);
                s.dragging = None;
                s.drag_moved = false;
                s.last_drag_offset = None;
                s.toggle_active = true;
                area_click.queue_draw();
                return;
            }
            s.toggle_active = false;
            s.dragging = Some((x, y));
            s.drag_moved = false;
            s.last_drag_offset = None;
        });

        let state_release = self.state.clone();
        click.connect_released(move |_, _n, x, y| {
            let mut s = state_release.borrow_mut();
            let was_drag = s.drag_moved;
            let from_toggle = s.toggle_active;
            s.dragging = None;
            s.drag_moved = false;
            s.last_drag_offset = None;
            s.toggle_active = false;
            if from_toggle || was_drag {
                return;
            }
            if let Some(index) = s.hit_annotation(x, y) {
                let annotation = s.annotations[index].clone();
                if let Some(cb) = &s.on_annotation_tapped {
                    cb(&annotation);
                }
            }
        });
        area.add_controller(click);

        // Drag to pan via GestureDrag.
        let drag = GestureDrag::new();
        drag.set_button(0);
        let state_begin = self.state.clone();
        drag.connect_drag_begin(move |_drag, x, y| {
            let mut s = state_begin.borrow_mut();
            if s.toggle_active {
                return;
            }
            if s.dragging.is_none() {
                s.dragging = Some((x, y));
                s.drag_moved = false;
                s.last_drag_offset = Some((0.0, 0.0));
            }
        });

        let state_pan = self.state.clone();
        let area_pan = area.clone();
        drag.connect_drag_update(move |_drag, dx, dy| {
            {
                let mut s = state_pan.borrow_mut();
                if s.toggle_active {
                    return;
                }
                if s.dragging.is_some() || s.drag_moved {
                    // GTK reports the cumulative offset from the drag start,
                    // so only the delta since the last event is applied.
                    let (last_dx, last_dy) = s.last_drag_offset.unwrap_or((0.0, 0.0));
                    let step_x = dx - last_dx;
                    let step_y = dy - last_dy;
                    s.last_drag_offset = Some((dx, dy));
                    if step_x == 0.0 && step_y == 0.0 {
                        return;
                    }
                    // Dragging right moves content right (viewport west);
                    // dragging down moves content down (viewport north).
                    s.camera.pan_pixels(step_x, -step_y);
                    s.drag_moved = true;
                    if let Some(cb) = &s.on_camera_changed {
                        cb(&s.camera);
                    }
                }
            }
            area_pan.queue_draw();
        });

        let state_end = self.state.clone();
        drag.connect_drag_end(move |_drag, _x, _y| {
            let mut s = state_end.borrow_mut();
            s.dragging = None;
            s.last_drag_offset = None;
        });
        area.add_controller(drag);

        // Scroll to zoom.
        let scroll = EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let state_scroll = self.state.clone();
        let area_scroll = area.clone();
        scroll.connect_scroll(move |_, _dx, dy| {
            let delta = if dy < 0.0 { 1.0 } else { -1.0 };
            {
                let mut s = state_scroll.borrow_mut();
                let (cx, cy) = (s.width / 2.0, s.height / 2.0);
                s.zoom_around(cx, cy, delta);
                if let Some(cb) = &s.on_camera_changed {
                    cb(&s.camera);
                }
            }
            area_scroll.queue_draw();
            glib::Propagation::Proceed
        });
        area.add_controller(scroll);

        // Double click to zoom in.
        let dblclick = GestureClick::new();
        dblclick.set_button(gtk::gdk::BUTTON_PRIMARY);
        let state_dbl = self.state.clone();
        let area_dbl = area.clone();
        dblclick.connect_pressed(move |gesture, n, x, y| {
            if n == 2 {
                let mut s = state_dbl.borrow_mut();
                s.zoom_around(x, y, 1.0);
                if let Some(cb) = &s.on_camera_changed {
                    cb(&s.camera);
                }
                area_dbl.queue_draw();
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        area.add_controller(dblclick);
    }

    // ─── Camera ────────────────────────────────────────────

    /// The current camera.
    pub fn camera(&self) -> MapCamera {
        self.state.borrow().camera
    }

    /// Replaces the camera and redraws.
    pub fn set_camera(&self, camera: MapCamera) {
        {
            let mut s = self.state.borrow_mut();
            s.camera = camera;
        }
        self.notify_camera_changed();
        self.area.queue_draw();
    }

    /// Centers the map at a zoom level.
    pub fn set_center(&self, center: Coordinate, zoom: f64) {
        self.set_camera(MapCamera::new(center, zoom));
    }

    /// The currently visible region.
    pub fn region(&self) -> MapRegion {
        let s = self.state.borrow();
        s.camera.region(s.width, s.height)
    }

    /// Fits the view to a region.
    pub fn set_region(&self, region: MapRegion) {
        let zoom = {
            let s = self.state.borrow();
            s.fitting_zoom(&region.bounding_box())
        };
        self.set_camera(MapCamera::new(region.center, zoom));
    }

    /// Fits the viewport to a bounding box.
    pub fn fit_bounds(&self, bbox: crate::types::BoundingBox) {
        let (center, zoom) = {
            let s = self.state.borrow();
            let center = Coordinate::new(
                (bbox.north + bbox.south) / 2.0,
                (bbox.east + bbox.west) / 2.0,
            );
            (center, s.fitting_zoom(&bbox))
        };
        self.set_camera(MapCamera::new(center, zoom));
    }

    // ─── Annotations ───────────────────────────────────────

    /// Adds an annotation pin.
    pub fn add_annotation(&self, annotation: Annotation) {
        self.state.borrow_mut().annotations.push(annotation);
        self.area.queue_draw();
    }

    /// Adds an annotation built from a place.
    pub fn add_place(&self, place: &crate::types::Place) {
        self.add_annotation(Annotation::from_place(place));
    }

    /// Removes an annotation by id.
    pub fn remove_annotation(&self, id: &str) {
        self.state
            .borrow_mut()
            .annotations
            .retain(|a| a.id != id);
        self.area.queue_draw();
    }

    /// Removes all annotations.
    pub fn clear_annotations(&self) {
        self.state.borrow_mut().annotations.clear();
        self.area.queue_draw();
    }

    /// All annotations.
    pub fn annotations(&self) -> Vec<Annotation> {
        self.state.borrow().annotations.clone()
    }

    /// Centers on a place and drops a pin.
    pub fn show_place(&self, place: &crate::types::Place) {
        self.clear_annotations();
        self.add_place(place);
        self.set_center(place.coordinate, self.camera().zoom.max(14.0));
    }

    // ─── Overlays ──────────────────────────────────────────

    /// Adds an overlay (polyline, polygon or circle).
    pub fn add_overlay(&self, overlay: impl Into<Overlay>) {
        self.state.borrow_mut().overlays.push(overlay.into());
        self.area.queue_draw();
    }

    /// Removes an overlay by id.
    pub fn remove_overlay(&self, id: &str) {
        self.state.borrow_mut().overlays.retain(|o| match o {
            Overlay::Polyline(p) => p.id != id,
            Overlay::Polygon(p) => p.id != id,
            Overlay::Circle(c) => c.id != id,
        });
        self.area.queue_draw();
    }

    /// Removes all overlays.
    pub fn clear_overlays(&self) {
        self.state.borrow_mut().overlays.clear();
        self.area.queue_draw();
    }

    /// Displays a route as an accent polyline and fits the viewport to it.
    pub fn display_route(&self, route: &Route) {
        self.clear_overlays();
        let line = Polyline::for_route(route);
        self.add_overlay(line);
        if let Some(bbox) = route
            .geometry
            .first()
            .zip(route.geometry.last())
            .and_then(|_| crate::overlays::bounding_box_of(&route.geometry))
        {
            self.fit_bounds(bbox);
        }
    }

    // ─── Style ─────────────────────────────────────────────

    /// Switches the base map style (also switches the tile provider).
    pub fn set_style(&self, style: MapStyle) {
        {
            let mut s = self.state.borrow_mut();
            s.config.style = style;
            s.tiles.clear();
            s.pending.clear();
        }
        self.area.queue_draw();
    }

    // ─── User Location ─────────────────────────────────────

    /// Shows the user location using CoreLocation and centers on it.
    ///
    /// CoreLocation is queried on a worker thread; when it answers, the blue
    /// user dot appears and the camera centers on it. Errors are logged via
    /// `MAPSKIT_DEBUG` only.
    pub fn show_user_location(&self) {
        let state = self.state.clone();
        let area = self.area.clone();
        let (sender, receiver) = async_channel::unbounded::<Result<Coordinate, String>>();

        std::thread::spawn(move || {
            let result = corelocation::get_location()
                .map(|loc| Coordinate::new(loc.coordinates.latitude, loc.coordinates.longitude))
                .map_err(|e| e.to_string());
            let _ = sender.send_blocking(result);
        });

        glib::spawn_future_local(async move {
            while let Ok(result) = receiver.recv().await {
                match result {
                    Ok(coordinate) => {
                        {
                            let mut s = state.borrow_mut();
                            s.user_location = Some(coordinate);
                            let zoom = s.camera.zoom.max(13.0);
                            s.camera = MapCamera::new(coordinate, zoom);
                            if let Some(cb) = &s.on_camera_changed {
                                cb(&s.camera);
                            }
                        }
                        area.queue_draw();
                    }
                    Err(e) => log_debug(&format!("user location failed: {e}")),
                }
            }
        });
    }

    /// The user location currently shown, if any.
    pub fn user_location(&self) -> Option<Coordinate> {
        self.state.borrow().user_location
    }

    /// Manually sets the shown user location (without querying CoreLocation).
    pub fn set_user_location(&self, coordinate: Coordinate) {
        {
            let mut s = self.state.borrow_mut();
            s.user_location = Some(coordinate);
        }
        self.area.queue_draw();
    }

    /// Hides the user location dot.
    pub fn hide_user_location(&self) {
        {
            let mut s = self.state.borrow_mut();
            s.user_location = None;
        }
        self.area.queue_draw();
    }

    // ─── Callbacks ─────────────────────────────────────────

    /// Sets the callback for annotation taps.
    pub fn on_annotation_tapped(&self, callback: impl Fn(&Annotation) + 'static) {
        self.state.borrow_mut().on_annotation_tapped = Some(Box::new(callback));
    }

    /// Sets the callback fired after camera changes (pan/zoom).
    pub fn on_camera_changed(&self, callback: impl Fn(&MapCamera) + 'static) {
        self.state.borrow_mut().on_camera_changed = Some(Box::new(callback));
    }

    fn notify_camera_changed(&self) {
        let s = self.state.borrow();
        if let Some(cb) = &s.on_camera_changed {
            cb(&s.camera);
        }
    }
}
