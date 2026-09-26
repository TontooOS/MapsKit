//! `MapView` — the interactive 2D map view, following Apple's `MKMapView`.
//!
//! The view renders slippy map tiles with Vello inside TontooUI, supports
//! pan (drag), zoom (scroll wheel) and draws annotations, polylines,
//! polygons and circles. Tiles are fetched on worker threads into shared
//! state; the TontooUI shell redraws continuously, so freshly fetched
//! tiles appear on the next frame without any explicit invalidation.
//!
//! `MapView` implements [`tontooui::elements::View`][tontooui-view] directly,
//! so it drops into any `VStack` / `HStack` / `ZStack` like a label or a
//! button:
//!
//! ```rust,no_run
//! use tontooui::elements::{View, VStack};
//! use mapskit::prelude::*;
//!
//! let mut map = MapView::new(&MapsConfiguration::new().style(MapStyle::Light));
//! map.set_center(Coordinate::new(52.52, 13.405), 12.0);
//! map.add_annotation(Annotation::new(Coordinate::new(52.52, 13.405), "Berlin"));
//!
//! let stack = VStack::new().child(map);
//! ```
//!
//! [tontooui-view]: https://docs.rs/tontooui/latest/tontooui/elements/layout/trait.View.html

use crate::camera::MapCamera;
use crate::config::{MapStyle, MapsConfiguration};
use crate::overlays::{Annotation, CircleOverlay, Overlay, Polyline};
use crate::providers::ProviderChain;
use crate::tiles::{self, TileCache, TileKey};
use crate::types::{Coordinate, MapRegion, Route};

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tontooui::elements::layout::View as TontooView;
use tontooui::renderer::images::ImageLoader;
use tontooui::renderer::text::{FontSystem, draw_layout};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Circle as KurboCircle, RoundedRect, Stroke};
use vello::peniko::{Brush, Color, Fill};

const TILE_SIZE: f64 = 256.0;
/// TontooOS orange accent (`#FF6B2B`) used for pins and routes.
const ACCENT: (f64, f64, f64) = (1.0, 0x6b as f64 / 255.0, 0x2b as f64 / 255.0);

/// Background color per style (TontooOS surfaces: dark `#1b2022`,
/// light `#ffffff`).
fn background_for_style(style: MapStyle) -> Color {
    match style {
        MapStyle::Dark | MapStyle::Satellite => Color::from_rgb8(0x1b, 0x20, 0x22),
        _ => Color::from_rgb8(0xff, 0xff, 0xff),
    }
}

fn rgba8(color: (f64, f64, f64, f64)) -> Color {
    Color::from_rgba8(
        (color.0.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.1.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.2.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.3.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn log_debug(message: &str) {
    if std::env::var("MAPSKIT_DEBUG").as_deref() == Ok("1") {
        eprintln!("[mapskit] {message}");
    }
}

type AnnotationCallback = Arc<dyn Fn(&Annotation) + Send + Sync>;
type CameraCallback = Arc<dyn Fn(&MapCamera) + Send + Sync>;

struct MapInner {
    config: MapsConfiguration,
    chain: Arc<ProviderChain>,
    camera: MapCamera,
    /// The currently rendered base map style (switchable at runtime).
    style: MapStyle,
    /// Raw PNG/JPEG tile bytes by key (`None` = failed, draws a placeholder).
    tiles: HashMap<TileKey, Option<Vec<u8>>>,
    pending: HashSet<TileKey>,
    annotations: Vec<Annotation>,
    overlays: Vec<Overlay>,
    /// Last mouse press in view-logical px (for tap vs. drag detection).
    press: Option<(f64, f64)>,
    /// Last hover position in view-logical px (for drag deltas).
    last_hover: Option<(f64, f64)>,
    drag_moved: bool,
    /// Screen rect of the layer toggle pill, refreshed on every frame.
    toggle_rect: Option<(f64, f64, f64, f64)>,
    /// Whether the active press started on the layer toggle.
    toggle_active: bool,
    width: f64,
    height: f64,
    offline: bool,
    /// The user's location, when shown via `show_user_location`.
    user_location: Option<Coordinate>,
    on_annotation_tapped: Option<AnnotationCallback>,
    on_camera_changed: Option<CameraCallback>,
}

impl MapInner {
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
        let lat = ((std::f64::consts::PI * (1.0 - 2.0 * wy / TILE_SIZE / n)).sinh())
            .atan()
            .to_degrees();
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
        let lat = ((std::f64::consts::PI * (1.0 - 2.0 * center_wy / TILE_SIZE / n)).sinh())
            .atan()
            .to_degrees();
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
        match self.toggle_rect {
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
///
/// Snapshots the wanted keys plus provider details under the lock, then
/// spawns one worker thread per tile. Threads write back into the shared
/// state directly; the TontooUI shell redraws continuously, so no explicit
/// invalidation is needed.
fn request_visible_tiles(shared: &Arc<Mutex<MapInner>>) {
    let (wanted, provider_name, cache_source, user_agent, timeout, cache_config) = {
        let Ok(s) = shared.lock() else { return };
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

    let chain = {
        let Ok(s) = shared.lock() else { return };
        s.chain.clone()
    };
    let style = {
        let Ok(s) = shared.lock() else { return };
        s.style
    };

    for key in wanted {
        {
            let Ok(mut s) = shared.lock() else { break };
            if s.pending.contains(&key) || s.tiles.contains_key(&key) {
                continue;
            }
            s.pending.insert(key);
        }

        let shared = shared.clone();
        let chain = chain.clone();
        let provider_name = provider_name.clone();
        let cache_source = cache_source.clone();
        let user_agent = user_agent.clone();
        let cache_config = cache_config.clone();
        std::thread::spawn(move || {
            let provider = chain.tile_provider_for(style).cloned().unwrap_or_else(|| {
                chain
                    .tile_provider_for(MapStyle::Standard)
                    .cloned()
                    .expect("chain empty")
            });
            let cache = TileCache::new(&cache_config, &provider_name, &cache_source);
            let result = tiles::fetch_tile(provider.as_ref(), &cache, key, &user_agent, timeout);
            if let Ok(mut s) = shared.lock() {
                s.pending.remove(&key);
                match result {
                    Ok(bytes) => {
                        s.offline = false;
                        s.tiles.insert(key, Some(bytes));
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
        });
    }
}

// ═══════════════════════════════════════════════════════════════
// Vello rendering (all geometry in physical px: logical * dpr)
// ═══════════════════════════════════════════════════════════════

/// Cache key for a tile inside TontooUI's [`ImageLoader`].
fn tile_image_key(provider: &str, source: &str, key: TileKey) -> String {
    format!("mapskit/{provider}/{source}/{}/{}/{}", key.z, key.x, key.y)
}

fn draw_tiles(
    inner: &MapInner,
    scene: &mut Scene,
    images: &mut ImageLoader<'_>,
    dpr: f64,
    ox: f64,
    oy: f64,
) {
    let z = inner.camera.zoom.round() as u8;
    let zoom_scale = (inner.camera.zoom - f64::from(z)).exp2();
    let tile_draw = TILE_SIZE * zoom_scale;

    let (cx, cy) = inner.world_px(inner.camera.center);
    let origin_x = cx - inner.width / 2.0;
    let origin_y = cy - inner.height / 2.0;

    let max_tiles = (1u32 << z) as i64;
    let min_tx = (origin_x / TILE_SIZE).floor().max(0.0) as i64;
    let max_tx = (((origin_x + inner.width) / TILE_SIZE).floor() as i64).min(max_tiles - 1);
    let min_ty = (origin_y / TILE_SIZE).floor().max(0.0) as i64;
    let max_ty = (((origin_y + inner.height) / TILE_SIZE).floor() as i64).min(max_tiles - 1);

    let provider = inner.tile_provider();
    let source = tiles::cache_source_key(provider.as_ref());
    let provider_name = provider.name().to_string();

    for ty in min_ty..=max_ty {
        for tx in min_tx..=max_tx {
            let key = TileKey::new(z, tx as u32, ty as u32);
            let lx = ox + (tx as f64 * TILE_SIZE - origin_x) * zoom_scale;
            let ly = oy + (ty as f64 * TILE_SIZE - origin_y) * zoom_scale;
            match inner.tiles.get(&key) {
                Some(Some(bytes)) => {
                    let cache_key = tile_image_key(&provider_name, &source, key);
                    if let Some((image, iw, _ih)) = images.raster(&cache_key, bytes, 512) {
                        let s = (tile_draw / iw as f64) * dpr;
                        let transform = Affine::translate((lx * dpr, ly * dpr)) * Affine::scale(s);
                        scene.draw_image(&image, transform);
                    }
                }
                _ => {
                    // Placeholder while loading.
                    let fill = match inner.style {
                        MapStyle::Dark | MapStyle::Satellite => {
                            Color::from_rgba8(255, 255, 255, 13)
                        }
                        _ => Color::from_rgba8(0, 0, 0, 15),
                    };
                    let rect = vello::kurbo::Rect::new(
                        lx * dpr,
                        ly * dpr,
                        (lx + tile_draw) * dpr,
                        (ly + tile_draw) * dpr,
                    );
                    scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(fill), None, &rect);
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

fn screen_path(points: &[(f64, f64)], ox: f64, oy: f64, dpr: f64, close: bool) -> BezPath {
    let mut path = BezPath::new();
    for (i, (sx, sy)) in points.iter().enumerate() {
        let (px, py) = ((ox + sx) * dpr, (oy + sy) * dpr);
        if i == 0 {
            path.move_to((px, py));
        } else {
            path.line_to((px, py));
        }
    }
    if close {
        path.close_path();
    }
    path
}

fn draw_overlays(inner: &MapInner, scene: &mut Scene, dpr: f64, ox: f64, oy: f64) {
    for overlay in &inner.overlays {
        match overlay {
            Overlay::Polyline(line) => {
                if line.points.len() < 2 {
                    continue;
                }
                let pts: Vec<(f64, f64)> =
                    line.points.iter().map(|p| inner.coord_to_screen(*p)).collect();
                let path = screen_path(&pts, ox, oy, dpr, false);
                let color = rgba8(parse_css_color(&line.color, ACCENT));
                scene.stroke(
                    &Stroke::new((line.width as f64 * dpr).max(1.0)),
                    Affine::IDENTITY,
                    &Brush::Solid(color),
                    None,
                    &path,
                );
            }
            Overlay::Polygon(polygon) => {
                if polygon.points.len() < 3 {
                    continue;
                }
                let pts: Vec<(f64, f64)> =
                    polygon.points.iter().map(|p| inner.coord_to_screen(*p)).collect();
                let path = screen_path(&pts, ox, oy, dpr, true);
                if !polygon.fill_color.is_empty() {
                    let color = rgba8(parse_css_color(&polygon.fill_color, ACCENT));
                    scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(color), None, &path);
                }
                if !polygon.stroke_color.is_empty() {
                    let color = rgba8(parse_css_color(&polygon.stroke_color, ACCENT));
                    scene.stroke(
                        &Stroke::new(2.0 * dpr),
                        Affine::IDENTITY,
                        &Brush::Solid(color),
                        None,
                        &path,
                    );
                }
            }
            Overlay::Circle(circle) => {
                draw_circle(inner, scene, circle, dpr, ox, oy);
            }
        }
    }
}

fn draw_circle(
    inner: &MapInner,
    scene: &mut Scene,
    circle: &CircleOverlay,
    dpr: f64,
    ox: f64,
    oy: f64,
) {
    // Convert the radius in meters to pixels at the center latitude.
    let z = inner.camera.zoom.round() as u8;
    let mpp = tiles::meters_per_pixel(z, circle.center.latitude);
    let scale = (inner.camera.zoom - f64::from(z)).exp2();
    let radius_px = circle.radius_m / mpp.max(0.01) * scale;
    let (sx, sy) = inner.coord_to_screen(circle.center);
    let shape = KurboCircle::new(((ox + sx) * dpr, (oy + sy) * dpr), radius_px.max(2.0) * dpr);

    if !circle.fill_color.is_empty() {
        let color = rgba8(parse_css_color(&circle.fill_color, (0.0, 0.9, 1.0)));
        scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(color), None, &shape);
    }
    if !circle.stroke_color.is_empty() {
        let color = rgba8(parse_css_color(&circle.stroke_color, (0.0, 0.9, 1.0)));
        scene.stroke(
            &Stroke::new(2.0 * dpr),
            Affine::IDENTITY,
            &Brush::Solid(color),
            None,
            &shape,
        );
    }
}

/// The blue user location dot with an accuracy halo.
fn draw_user_location(inner: &MapInner, scene: &mut Scene, dpr: f64, ox: f64, oy: f64) {
    let Some(coordinate) = inner.user_location else {
        return;
    };
    let (sx, sy) = inner.coord_to_screen(coordinate);
    let (px, py) = ((ox + sx) * dpr, (oy + sy) * dpr);

    let z = inner.camera.zoom.round() as u8;
    let mpp = tiles::meters_per_pixel(z, coordinate.latitude);
    let scale = (inner.camera.zoom - f64::from(z)).exp2();
    let halo_px = (50.0 / mpp.max(0.5) * scale).clamp(12.0, 200.0) * dpr;
    let halo = KurboCircle::new((px, py), halo_px);
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Solid(Color::from_rgba8(26, 115, 242, 38)),
        None,
        &halo,
    );

    let ring = KurboCircle::new((px, py), 7.0 * dpr);
    scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(Color::WHITE), None, &ring);
    let core = KurboCircle::new((px, py), 5.5 * dpr);
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Solid(Color::from_rgb8(26, 128, 255)),
        None,
        &core,
    );
}

fn draw_annotations(
    inner: &MapInner,
    scene: &mut Scene,
    fonts: &mut FontSystem,
    dpr: f64,
    ox: f64,
    oy: f64,
) {
    for annotation in &inner.annotations {
        let (sx, sy) = inner.coord_to_screen(annotation.coordinate);
        if sx < -40.0 || sy < -60.0 || sx > inner.width + 40.0 || sy > inner.height + 60.0 {
            continue;
        }
        let (px, py) = ((ox + sx) * dpr, (oy + sy) * dpr);

        let (ar, ag, ab) = if annotation.selected {
            (255u8, 140u8, 89u8)
        } else {
            (255, 0x6b, 0x2b)
        };
        let pin_color = Color::from_rgb8(ar, ag, ab);

        // Pin shadow.
        let shadow = KurboCircle::new((px, (oy + sy - 14.0) * dpr), 7.5 * dpr);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(0, 0, 0, 64)),
            None,
            &shadow,
        );
        // Pin head.
        let head = KurboCircle::new((px, (oy + sy - 14.0) * dpr), 7.0 * dpr);
        scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(pin_color), None, &head);
        // Pin tip.
        let mut tip = BezPath::new();
        tip.move_to(((ox + sx - 3.0) * dpr, (oy + sy - 9.0) * dpr));
        tip.line_to((px, py));
        tip.line_to(((ox + sx + 3.0) * dpr, (oy + sy - 9.0) * dpr));
        tip.close_path();
        scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(pin_color), None, &tip);

        // Label plate.
        if !annotation.title.is_empty() {
            let layout = fonts.layout_text(&annotation.title, 12.0, Color::WHITE, None);
            let (lw, lh) = FontSystem::layout_size(&layout);
            let (lw, lh) = (lw / fonts.scale, lh / fonts.scale);
            let pad_x: f32 = 5.0;
            let pad_y: f32 = 3.0;
            let bx = (ox + sx + 10.0) * dpr - pad_x as f64 * dpr;
            let by = (oy + sy - 20.0) * dpr - pad_y as f64 * dpr;
            let plate = RoundedRect::new(
                bx,
                by,
                bx + (lw + pad_x * 2.0) as f64 * dpr,
                by + (lh + pad_y * 2.0) as f64 * dpr,
                5.0 * dpr,
            );
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                &Brush::Solid(Color::from_rgba8(0, 0, 0, 140)),
                None,
                &plate,
            );
            draw_layout(
                scene,
                &layout,
                (ox + sx + 10.0) as f32,
                (oy + sy - 20.0) as f32,
                fonts.scale,
            );
        }
    }
}

/// Draws a small pill badge with text; returns its logical rect.
fn draw_pill(
    scene: &mut Scene,
    fonts: &mut FontSystem,
    text: &str,
    size: f32,
    x: f32,
    y: f32,
    fill: Color,
    fg: Color,
) -> (f32, f32, f32, f32) {
    let layout = fonts.layout_text(text, size, fg, None);
    let (lw, lh) = FontSystem::layout_size(&layout);
    let (lw, lh) = (lw / fonts.scale, lh / fonts.scale);
    let pad_x = 6.0;
    let pad_y = 3.0;
    let dpr = fonts.scale as f64;
    let plate = RoundedRect::new(
        x as f64 * dpr,
        y as f64 * dpr,
        (x + lw + pad_x * 2.0) as f64 * dpr,
        (y + lh + pad_y * 2.0) as f64 * dpr,
        4.0 * dpr,
    );
    scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(fill), None, &plate);
    draw_layout(scene, &layout, x + pad_x, y + pad_y, fonts.scale);
    (x, y, lw + pad_x * 2.0, lh + pad_y * 2.0)
}

fn draw_badges(
    inner: &MapInner,
    scene: &mut Scene,
    fonts: &mut FontSystem,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
) {
    // Attribution bottom-right.
    let attribution =
        crate::lang::t_or(inner.attribution_key(), "\u{a9} OpenStreetMap contributors");
    let layout = fonts.layout_text(&attribution, 10.0, Color::from_rgba8(255, 255, 255, 242), None);
    let (lw, lh) = FontSystem::layout_size(&layout);
    let (lw, lh) = (lw / fonts.scale, lh / fonts.scale);
    let pad = 6.0;
    let bw = lw + pad * 2.0;
    let bh = lh + pad;
    let bx = ox + w - bw - 4.0;
    let by = oy + h - bh - 4.0;
    let dpr = fonts.scale as f64;
    let plate = RoundedRect::new(
        bx as f64 * dpr,
        by as f64 * dpr,
        (bx + bw) as f64 * dpr,
        (by + bh) as f64 * dpr,
        4.0 * dpr,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Solid(Color::from_rgba8(0, 0, 0, 115)),
        None,
        &plate,
    );
    draw_layout(scene, &layout, bx + pad, by + pad / 2.0, fonts.scale);

    // Offline badge top-left.
    if inner.offline {
        let text = crate::lang::t_or("mapskit.map.offline", "Offline");
        draw_pill(
            scene,
            fonts,
            &text,
            10.0,
            ox + 6.0,
            oy + 6.0,
            Color::from_rgba8(140, 38, 13, 217),
            Color::from_rgba8(255, 255, 255, 242),
        );
    }
}

/// The pill button label: shows the layer the tap switches to.
fn layer_toggle_label(inner: &MapInner) -> String {
    if inner.style == MapStyle::Satellite {
        crate::lang::t_or("mapskit.map.standard", "Standard")
    } else {
        crate::lang::t_or("mapskit.map.satellite", "Satellite")
    }
}

/// Draws the layer toggle pill at the bottom left and returns its logical rect.
fn draw_layer_toggle(
    inner: &MapInner,
    scene: &mut Scene,
    fonts: &mut FontSystem,
    ox: f32,
    oy: f32,
    h: f32,
) -> Option<(f64, f64, f64, f64)> {
    let label = layer_toggle_label(inner);
    let layout = fonts.layout_text(
        &label,
        11.0,
        match inner.style {
            MapStyle::Dark | MapStyle::Satellite => Color::WHITE,
            _ => Color::from_rgb8(0x27, 0x27, 0x27),
        },
        None,
    );
    let (lw, lh) = FontSystem::layout_size(&layout);
    let (lw, lh) = (lw / fonts.scale, lh / fonts.scale);

    const PAD_X: f32 = 12.0;
    const PAD_Y: f32 = 7.0;
    const MARGIN: f32 = 10.0;
    let w = lw + PAD_X * 2.0;
    let ph = lh + PAD_Y * 2.0;
    let x = ox + MARGIN;
    let y = oy + h - ph - MARGIN;
    let dpr = fonts.scale as f64;

    let fill_alpha: u8 = match inner.style {
        MapStyle::Dark | MapStyle::Satellite => 31,
        _ => 217,
    };
    let plate = RoundedRect::new(
        x as f64 * dpr,
        y as f64 * dpr,
        (x + w) as f64 * dpr,
        (y + ph) as f64 * dpr,
        (ph / 2.0) as f64 * dpr,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Solid(Color::from_rgba8(0, 0, 0, fill_alpha)),
        None,
        &plate,
    );
    let border = RoundedRect::new(
        (x as f64 + 0.5) * dpr,
        (y as f64 + 0.5) * dpr,
        ((x + w) as f64 - 0.5) * dpr,
        ((y + ph) as f64 - 0.5) * dpr,
        (ph / 2.0) as f64 * dpr,
    );
    let line_color = match inner.style {
        MapStyle::Dark | MapStyle::Satellite => Color::from_rgba8(255, 255, 255, 46),
        _ => Color::from_rgba8(0, 0, 0, 46),
    };
    scene.stroke(
        &Stroke::new(1.0 * dpr),
        Affine::IDENTITY,
        &Brush::Solid(line_color),
        None,
        &border,
    );
    draw_layout(scene, &layout, x + PAD_X, y + PAD_Y, fonts.scale);

    Some((x as f64 - ox as f64, y as f64 - oy as f64, w as f64, ph as f64))
}

// ═══════════════════════════════════════════════════════════════
// MapView
// ═══════════════════════════════════════════════════════════════

/// An interactive 2D map view for TontooUI.
///
/// Implements [`TontooView`], so it embeds into any stack. The view shares
/// its state through an `Arc<Mutex<..>>` with tile worker threads; it is
/// `Clone` (clones share the same map) and all state-changing methods take
/// `&self`.
///
/// Host `App`s must forward `mouse_down` / `mouse_up` / `mouse_wheel` and
/// `mouse_move` (as `set_hover`) for pan and zoom to work.
#[derive(Clone)]
pub struct MapView {
    shared: Arc<Mutex<MapInner>>,
    x: f32,
    y: f32,
    placed_w: f32,
    placed_h: f32,
}

impl MapView {
    /// Creates a map view from the configuration.
    pub fn new(config: &MapsConfiguration) -> Self {
        Self {
            shared: Arc::new(Mutex::new(MapInner {
                config: config.clone(),
                chain: Arc::new(ProviderChain::with_config(config)),
                camera: MapCamera::default(),
                style: config.style,
                tiles: HashMap::new(),
                pending: HashSet::new(),
                annotations: Vec::new(),
                overlays: Vec::new(),
                press: None,
                last_hover: None,
                drag_moved: false,
                toggle_rect: None,
                toggle_active: false,
                width: 400.0,
                height: 300.0,
                offline: false,
                user_location: None,
                on_annotation_tapped: None,
                on_camera_changed: None,
            })),
            x: 0.0,
            y: 0.0,
            placed_w: 400.0,
            placed_h: 300.0,
        }
    }

    /// Placed rect (x, y, width, height) in logical px.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.placed_w, self.placed_h)
    }

    fn notify_camera_changed(&self) {
        let (callback, camera) = match self.shared.lock() {
            Ok(s) => (s.on_camera_changed.clone(), s.camera),
            Err(_) => return,
        };
        if let Some(cb) = callback {
            cb(&camera);
        }
    }

    /// Switches the base layer to satellite imagery (or back to the
    /// configured base style). Cached tiles of the old layer are dropped.
    pub fn set_satellite(&self, enabled: bool) {
        let style = {
            let s = match self.shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if enabled {
                MapStyle::Satellite
            } else {
                s.config.style
            }
        };
        if let Ok(mut s) = self.shared.lock() {
            s.set_base_style(style);
        }
    }

    /// Whether satellite imagery is currently shown.
    pub fn is_satellite(&self) -> bool {
        self.shared.lock().map(|s| s.style == MapStyle::Satellite).unwrap_or(false)
    }

    // ─── Camera ────────────────────────────────────────────

    /// The current camera.
    pub fn camera(&self) -> MapCamera {
        self.shared.lock().map(|s| s.camera).unwrap_or_default()
    }

    /// Replaces the camera and notifies the camera callback.
    pub fn set_camera(&self, camera: MapCamera) {
        if let Ok(mut s) = self.shared.lock() {
            s.camera = camera;
        }
        self.notify_camera_changed();
    }

    /// Centers the map at a zoom level.
    pub fn set_center(&self, center: Coordinate, zoom: f64) {
        self.set_camera(MapCamera::new(center, zoom));
    }

    /// The currently visible region.
    pub fn region(&self) -> MapRegion {
        match self.shared.lock() {
            Ok(s) => s.camera.region(s.width, s.height),
            Err(_) => MapCamera::default().region(400.0, 300.0),
        }
    }

    /// Fits the view to a region.
    pub fn set_region(&self, region: MapRegion) {
        let zoom = match self.shared.lock() {
            Ok(s) => s.fitting_zoom(&region.bounding_box()),
            Err(_) => return,
        };
        self.set_camera(MapCamera::new(region.center, zoom));
    }

    /// Fits the viewport to a bounding box.
    pub fn fit_bounds(&self, bbox: crate::types::BoundingBox) {
        let (center, zoom) = match self.shared.lock() {
            Ok(s) => {
                let center = Coordinate::new(
                    (bbox.north + bbox.south) / 2.0,
                    (bbox.east + bbox.west) / 2.0,
                );
                (center, s.fitting_zoom(&bbox))
            }
            Err(_) => return,
        };
        self.set_camera(MapCamera::new(center, zoom));
    }

    // ─── Annotations ───────────────────────────────────────

    /// Adds an annotation pin.
    pub fn add_annotation(&self, annotation: Annotation) {
        if let Ok(mut s) = self.shared.lock() {
            s.annotations.push(annotation);
        }
    }

    /// Adds an annotation built from a place.
    pub fn add_place(&self, place: &crate::types::Place) {
        self.add_annotation(Annotation::from_place(place));
    }

    /// Removes an annotation by id.
    pub fn remove_annotation(&self, id: &str) {
        if let Ok(mut s) = self.shared.lock() {
            s.annotations.retain(|a| a.id != id);
        }
    }

    /// Removes all annotations.
    pub fn clear_annotations(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.annotations.clear();
        }
    }

    /// All annotations.
    pub fn annotations(&self) -> Vec<Annotation> {
        self.shared.lock().map(|s| s.annotations.clone()).unwrap_or_default()
    }

    /// Centers on a place and drops a pin.
    pub fn show_place(&self, place: &crate::types::Place) {
        self.clear_annotations();
        self.add_place(place);
        let zoom = self.camera().zoom.max(14.0);
        self.set_center(place.coordinate, zoom);
    }

    // ─── Overlays ──────────────────────────────────────────

    /// Adds an overlay (polyline, polygon or circle).
    pub fn add_overlay(&self, overlay: impl Into<Overlay>) {
        if let Ok(mut s) = self.shared.lock() {
            s.overlays.push(overlay.into());
        }
    }

    /// Removes an overlay by id.
    pub fn remove_overlay(&self, id: &str) {
        if let Ok(mut s) = self.shared.lock() {
            s.overlays.retain(|o| match o {
                Overlay::Polyline(p) => p.id != id,
                Overlay::Polygon(p) => p.id != id,
                Overlay::Circle(c) => c.id != id,
            });
        }
    }

    /// Removes all overlays.
    pub fn clear_overlays(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.overlays.clear();
        }
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
    /// Cached tiles of the old style are dropped.
    pub fn set_style(&self, style: MapStyle) {
        if let Ok(mut s) = self.shared.lock() {
            s.config.style = style;
            s.set_base_style(style);
        }
    }

    // ─── User Location ─────────────────────────────────────

    /// Shows the user location using CoreLocation and centers on it.
    ///
    /// CoreLocation is queried on a worker thread; when it answers, the blue
    /// user dot appears and the camera centers on it. Errors are logged via
    /// `MAPSKIT_DEBUG` only.
    pub fn show_user_location(&self) {
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let result = corelocation::get_location()
                .map(|loc| Coordinate::new(loc.coordinates.latitude, loc.coordinates.longitude))
                .map_err(|e| e.to_string());
            match result {
                Ok(coordinate) => {
                    let callback = {
                        match shared.lock() {
                            Ok(mut s) => {
                                s.user_location = Some(coordinate);
                                let zoom = s.camera.zoom.max(13.0);
                                s.camera = MapCamera::new(coordinate, zoom);
                                s.on_camera_changed.clone().map(|cb| (cb, s.camera))
                            }
                            Err(_) => None,
                        }
                    };
                    if let Some((cb, camera)) = callback {
                        cb(&camera);
                    }
                }
                Err(e) => log_debug(&format!("user location failed: {e}")),
            }
        });
    }

    /// The user location currently shown, if any.
    pub fn user_location(&self) -> Option<Coordinate> {
        self.shared.lock().ok().and_then(|s| s.user_location)
    }

    /// Manually sets the shown user location (without querying CoreLocation).
    pub fn set_user_location(&self, coordinate: Coordinate) {
        if let Ok(mut s) = self.shared.lock() {
            s.user_location = Some(coordinate);
        }
    }

    /// Hides the user location dot.
    pub fn hide_user_location(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.user_location = None;
        }
    }

    // ─── Callbacks ─────────────────────────────────────────

    /// Sets the callback for annotation taps.
    pub fn on_annotation_tapped(&self, callback: impl Fn(&Annotation) + Send + Sync + 'static) {
        if let Ok(mut s) = self.shared.lock() {
            s.on_annotation_tapped = Some(Arc::new(callback));
        }
    }

    /// Sets the callback fired after camera changes (pan/zoom).
    pub fn on_camera_changed(&self, callback: impl Fn(&MapCamera) + Send + Sync + 'static) {
        if let Ok(mut s) = self.shared.lock() {
            s.on_camera_changed = Some(Arc::new(callback));
        }
    }

    // ─── Pointer handling (called by the host App) ─────────

    fn press_at(&mut self, x: f64, y: f64) {
        let lx = x - self.x as f64;
        let ly = y - self.y as f64;
        if let Ok(mut s) = self.shared.lock() {
            if s.toggle_hit(lx, ly) {
                let next = if s.style == MapStyle::Satellite {
                    s.config.style
                } else {
                    MapStyle::Satellite
                };
                s.set_base_style(next);
                s.press = None;
                s.drag_moved = false;
                s.toggle_active = true;
                return;
            }
            s.toggle_active = false;
            s.press = Some((lx, ly));
            s.last_hover = Some((lx, ly));
            s.drag_moved = false;
        }
    }

    fn release_at(&mut self, x: f64, y: f64) {
        let lx = x - self.x as f64;
        let ly = y - self.y as f64;
        let (tapped, callback, was_drag, from_toggle) = match self.shared.lock() {
            Ok(mut s) => {
                let was_drag = s.drag_moved;
                let from_toggle = s.toggle_active;
                s.press = None;
                s.last_hover = None;
                s.drag_moved = false;
                s.toggle_active = false;
                if from_toggle || was_drag {
                    (None, None, true, from_toggle)
                } else {
                    let hit = s.hit_annotation(lx, ly).map(|i| s.annotations[i].clone());
                    let cb = s.on_annotation_tapped.clone();
                    (hit, cb, false, false)
                }
            }
            Err(_) => (None, None, false, false),
        };
        let _ = (was_drag, from_toggle);
        if let (Some(annotation), Some(cb)) = (tapped, callback) {
            cb(&annotation);
        }
    }

    fn hover_at(&mut self, x: f32, y: f32) {
        let lx = x as f64 - self.x as f64;
        let ly = y as f64 - self.y as f64;
        let callback = {
            match self.shared.lock() {
                Ok(mut s) => {
                    let prev = s.last_hover;
                    s.last_hover = Some((lx, ly));
                    if s.toggle_active || s.press.is_none() {
                        None
                    } else if let Some((px, py)) = prev {
                        let (dx, dy) = (lx - px, ly - py);
                        if dx == 0.0 && dy == 0.0 {
                            None
                        } else {
                            // Dragging right moves content right (viewport
                            // west); dragging down moves content down
                            // (viewport north).
                            s.camera.pan_pixels(dx, -dy);
                            s.drag_moved = true;
                            s.on_camera_changed.clone().map(|cb| (cb, s.camera))
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        };
        if let Some((cb, camera)) = callback {
            cb(&camera);
        }
    }

    fn wheel_at(&mut self, dx: f64, dy: f64) {
        let _ = dx;
        // TontooUI reports scroll deltas in logical px (right/down
        // positive); one notch (~20 px) is one zoom level.
        let delta = (-dy / 20.0).clamp(-3.0, 3.0);
        if delta == 0.0 {
            return;
        }
        let callback = {
            match self.shared.lock() {
                Ok(mut s) => {
                    let (ax, ay) = s.last_hover.unwrap_or((s.width / 2.0, s.height / 2.0));
                    s.zoom_around(ax, ay, delta);
                    s.on_camera_changed.clone().map(|cb| (cb, s.camera))
                }
                Err(_) => None,
            }
        };
        if let Some((cb, camera)) = callback {
            cb(&camera);
        }
    }
}

impl TontooView for MapView {
    fn measure(&mut self, _fonts: &mut FontSystem) -> (f32, f32) {
        (self.placed_w.max(200.0), self.placed_h.max(150.0))
    }

    fn place(&mut self, _fonts: &mut FontSystem, x: f32, y: f32, width: f32, height: f32) {
        self.x = x;
        self.y = y;
        self.placed_w = width.max(1.0);
        self.placed_h = height.max(1.0);
        if let Ok(mut s) = self.shared.lock() {
            s.width = self.placed_w as f64;
            s.height = self.placed_h as f64;
        }
    }

    fn draw(&mut self, scene: &mut Scene, fonts: &mut FontSystem, images: &mut ImageLoader<'_>) {
        if self.placed_w <= 0.0 || self.placed_h <= 0.0 {
            return;
        }
        request_visible_tiles(&self.shared);

        let dpr = fonts.scale as f64;
        let (ox, oy) = (self.x as f64, self.y as f64);

        // Clip everything to the view rect.
        let clip = vello::kurbo::Rect::new(
            ox * dpr,
            oy * dpr,
            (ox + self.placed_w as f64) * dpr,
            (oy + self.placed_h as f64) * dpr,
        );
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);

        let Ok(mut inner) = self.shared.lock() else {
            scene.pop_layer();
            return;
        };
        let bg = background_for_style(inner.style);
        scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(bg), None, &clip);

        draw_tiles(&inner, scene, images, dpr, ox, oy);
        draw_overlays(&inner, scene, dpr, ox, oy);
        draw_user_location(&inner, scene, dpr, ox, oy);
        draw_annotations(&inner, scene, fonts, dpr, ox, oy);
        draw_badges(&inner, scene, fonts, self.x, self.y, self.placed_w, self.placed_h);
        let toggle_rect = draw_layer_toggle(&inner, scene, fonts, self.x, self.y, self.placed_h);
        inner.toggle_rect = toggle_rect;

        scene.pop_layer();
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.press_at(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.release_at(x, y);
    }

    fn set_hover(&mut self, x: f32, y: f32) {
        self.hover_at(x, y);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        self.wheel_at(dx, dy);
    }

    fn flex(&self) -> f32 {
        1.0
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
