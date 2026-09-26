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
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

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

/// Duration of one animated zoom step.
const ZOOM_ANIM_DURATION: Duration = Duration::from_millis(200);

/// Ease-out cubic: fast start, soft landing.
fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Interpolates two coordinates, taking the shortest path around the
/// antimeridian for longitude.
fn lerp_coord(from: Coordinate, to: Coordinate, t: f64) -> Coordinate {
    let t = t.clamp(0.0, 1.0);
    let lat = from.latitude + (to.latitude - from.latitude) * t;
    let dlon = (to.longitude - from.longitude + 540.0).rem_euclid(360.0) - 180.0;
    let lon = (from.longitude + dlon * t + 180.0).rem_euclid(360.0) - 180.0;
    Coordinate::new(lat, lon)
}

/// The coordinate under a screen point for an explicit camera and
/// viewport size.
fn screen_to_coord_at(
    center: Coordinate,
    zoom: f64,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> Coordinate {
    let z = zoom.round() as u8;
    let scale = (zoom - f64::from(z)).exp2();
    let (fx, fy) = tiles::tile_for(center, z);
    let (cx, cy) = (fx * TILE_SIZE, fy * TILE_SIZE);
    let origin_x = cx - width / 2.0;
    let origin_y = cy - height / 2.0;
    let wx = x / scale + origin_x;
    let wy = y / scale + origin_y;
    let n = f64::from(1u32 << z);
    let lon = wx / TILE_SIZE / n * 360.0 - 180.0;
    let lat = ((std::f64::consts::PI * (1.0 - 2.0 * wy / TILE_SIZE / n)).sinh())
        .atan()
        .to_degrees();
    Coordinate::new(lat.clamp(-85.051_128_78, 85.051_128_78), lon)
}

/// One animated zoom step from camera A to camera B.
struct ZoomAnim {
    from_center: Coordinate,
    from_zoom: f64,
    to_center: Coordinate,
    to_zoom: f64,
    start: Instant,
}

impl ZoomAnim {
    /// Linear progress in `0..=1`.
    fn progress(&self, now: Instant) -> f64 {
        (now.duration_since(self.start).as_secs_f64() / ZOOM_ANIM_DURATION.as_secs_f64())
            .clamp(0.0, 1.0)
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
    /// Raw PNG/JPEG tile bytes by key.
    tiles: HashMap<TileKey, Vec<u8>>,
    pending: HashSet<TileKey>,
    /// Failed tiles with the time of the last attempt. Entries are
    /// retried after [`TILE_RETRY_AFTER`]; the offline badge shows while
    /// any currently visible tile has a recent failure.
    failed: HashMap<TileKey, Instant>,
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
    /// Running zoom animation, advanced every frame in `draw`.
    zoom_anim: Option<ZoomAnim>,
    /// The user's location, when shown via `show_user_location`.
    user_location: Option<Coordinate>,
    /// Last CoreLocation fix with its timestamp. Reused by
    /// `show_user_location` while fresh so repeated presses do not
    /// re-query the daemon every time.
    location_cache: Option<(Coordinate, Instant)>,
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
        // Attribution follows the active imagery, not the provider name:
        // both Esri services attribute Esri, everything else is
        // OpenStreetMap data.
        match self.style {
            MapStyle::Satellite => "mapskit.map.attribution.esri",
            MapStyle::Light => "mapskit.map.attribution.esri_street",
            MapStyle::Dark => "mapskit.map.attribution.esri_dark",
            _ => "mapskit.map.attribution.osm",
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
        screen_to_coord_at(self.camera.center, self.camera.zoom, self.width, self.height, x, y)
    }

    /// Zoom target around a screen anchor so the coordinate under it stays
    /// put, computed from an explicit camera (used to retarget mid-flight).
    fn zoom_target(
        &self,
        from_center: Coordinate,
        from_zoom: f64,
        x: f64,
        y: f64,
        delta: f64,
    ) -> (Coordinate, f64) {
        let anchor = screen_to_coord_at(from_center, from_zoom, self.width, self.height, x, y);
        let new_zoom = (from_zoom + delta).clamp(0.0, f64::from(tiles::MAX_ZOOM));
        if (new_zoom - from_zoom).abs() < f64::EPSILON {
            return (from_center, from_zoom);
        }

        // Shift the center so the anchor stays under (x, y).
        let z = new_zoom.round() as u8;
        let scale = (new_zoom - f64::from(z)).exp2();
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
        (
            Coordinate::new(
                lat.clamp(-85.051_128_78, 85.051_128_78),
                lon.clamp(-180.0, 180.0),
            ),
            new_zoom,
        )
    }

    /// Camera actually on screen right now: the animation interpolation
    /// while a zoom is running, else the stored camera.
    fn displayed_camera(&self, now: Instant) -> (Coordinate, f64) {
        let Some(anim) = &self.zoom_anim else {
            return (self.camera.center, self.camera.zoom);
        };
        let t = anim.progress(now);
        (
            lerp_coord(anim.from_center, anim.to_center, ease_out_cubic(t)),
            anim.from_zoom + (anim.to_zoom - anim.from_zoom) * ease_out_cubic(t),
        )
    }

    /// Starts (or retargets) an animated zoom step around a screen anchor.
    /// New wheel events mid-flight retarget from the currently displayed
    /// state, so fast scrolling accelerates smoothly instead of jumping.
    fn begin_zoom_step(&mut self, x: f64, y: f64, delta: f64) {
        let now = Instant::now();
        let (from_center, from_zoom) = self.displayed_camera(now);
        let (to_center, to_zoom) = self.zoom_target(from_center, from_zoom, x, y, delta);
        if (to_zoom - from_zoom).abs() < f64::EPSILON {
            return;
        }
        // Snap the stored camera to the displayed state; the animation
        // interpolates from there.
        self.camera.center = from_center;
        self.camera.zoom = from_zoom;
        self.zoom_anim = Some(ZoomAnim {
            from_center,
            from_zoom,
            to_center,
            to_zoom,
            start: now,
        });
    }

    /// Advances a running zoom animation to `now`. Returns the camera
    /// callback payload once the animation completes.
    fn advance_anim(&mut self, now: Instant) -> Option<(CameraCallback, MapCamera)> {
        let anim = self.zoom_anim.take()?;
        let t = anim.progress(now);
        let e = ease_out_cubic(t);
        self.camera.center = lerp_coord(anim.from_center, anim.to_center, e);
        self.camera.zoom = anim.from_zoom + (anim.to_zoom - anim.from_zoom) * e;
        if t >= 1.0 {
            self.zoom_anim = None;
            self.on_camera_changed.clone().map(|cb| (cb, self.camera))
        } else {
            self.zoom_anim = Some(anim);
            None
        }
    }

    /// Cancels a running zoom animation, snapping to the displayed state
    /// so pans and presses take over seamlessly.
    fn cancel_anim(&mut self, now: Instant) {
        if self.zoom_anim.is_some() {
            let (center, zoom) = self.displayed_camera(now);
            self.camera.center = center;
            self.camera.zoom = zoom;
            self.zoom_anim = None;
        }
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
        self.failed.clear();
        log_debug(&format!("base layer switched to {style:?}"));
    }
}

/// Requests all visible tiles that are not loaded yet.
/// Number of concurrent tile downloads. Tile servers rate-limit parallel
/// bursts (HTTP 429), so fetches go through a small shared worker pool
/// instead of one thread per tile.
const TILE_WORKERS: usize = 6;
/// How long a CoreLocation fix is reused before `show_user_location`
/// queries again.
const LOCATION_CACHE_TTL: Duration = Duration::from_secs(60);
/// Failed tiles are retried after this cooldown instead of staying blank
/// forever.
const TILE_RETRY_AFTER: Duration = Duration::from_secs(8);
/// Extra tile ring loaded around the viewport so panning never shows
/// placeholders. Center-first ordering keeps visible tiles ahead of the
/// prefetch ring.
const PREFETCH_MARGIN: i64 = 1;

/// One tile download for the shared worker pool.
struct TileJob {
    chain: Arc<ProviderChain>,
    style: MapStyle,
    provider_name: String,
    cache_source: String,
    user_agent: String,
    timeout_seconds: u64,
    cache_config: MapsConfiguration,
    key: TileKey,
    /// Weak so a dropped view never keeps jobs (or workers) alive.
    target: Weak<Mutex<MapInner>>,
}

static TILE_POOL: OnceLock<std::sync::mpsc::Sender<TileJob>> = OnceLock::new();

/// The process-wide tile download pool: [`TILE_WORKERS`] threads draining
/// one queue, shared by all map views.
fn tile_pool() -> &'static std::sync::mpsc::Sender<TileJob> {
    TILE_POOL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<TileJob>();
        let rx = Arc::new(Mutex::new(rx));
        for worker in 0..TILE_WORKERS {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("mapskit-tiles-{worker}"))
                .spawn(move || loop {
                    let job = { rx.lock().expect("tile queue poisoned").recv() };
                    let Ok(job) = job else { return };
                    let provider = job
                        .chain
                        .tile_provider_for(job.style)
                        .cloned()
                        .unwrap_or_else(|| {
                            job.chain
                                .tile_provider_for(MapStyle::Standard)
                                .cloned()
                                .expect("chain empty")
                        });
                    let cache =
                        TileCache::new(&job.cache_config, &job.provider_name, &job.cache_source);
                    let result = tiles::fetch_tile(
                        provider.as_ref(),
                        &cache,
                        job.key,
                        &job.user_agent,
                        job.timeout_seconds,
                    );
                    let Some(shared) = job.target.upgrade() else {
                        continue;
                    };
                    let Ok(mut s) = shared.lock() else {
                        continue;
                    };
                    // The layer may have changed while downloading; results
                    // of the old layer must not pollute the new one (its
                    // caches were cleared on switch).
                    if s.style != job.style {
                        continue;
                    }
                    s.pending.remove(&job.key);
                    match result {
                        Ok(bytes) => {
                            s.tiles.insert(job.key, bytes);
                            s.failed.remove(&job.key);
                        }
                        Err(e) => {
                            s.failed.insert(job.key, Instant::now());
                            log_debug(&format!(
                                "tile {}/{}/{} failed: {}",
                                job.key.z, job.key.x, job.key.y, e
                            ));
                        }
                    }
                })
                .expect("tile worker spawn failed");
        }
        tx
    })
}

/// Visible tile range for the current camera and viewport.
fn visible_tile_range(
    center: Coordinate,
    zoom: u8,
    zoom_scale: f64,
    width: f64,
    height: f64,
) -> (i64, i64, i64, i64) {
    let view_tiles_x = width / (TILE_SIZE * zoom_scale);
    let view_tiles_y = height / (TILE_SIZE * zoom_scale);
    let (cfx, cfy) = tiles::tile_for(center, zoom);
    let max_tiles = (1i64 << zoom) - 1;
    let min_x = ((cfx - view_tiles_x / 2.0).floor() as i64).clamp(0, max_tiles);
    let max_x = ((cfx + view_tiles_x / 2.0).floor() as i64).clamp(0, max_tiles);
    let min_y = (cfy - view_tiles_y / 2.0).floor().max(0.0) as i64;
    let max_y = ((cfy + view_tiles_y / 2.0).floor() as i64).min(max_tiles);
    (min_x, max_x, min_y, max_y)
}

/// Requests all visible tiles that are not loaded yet.
///
/// Snapshots the wanted keys plus provider details under the lock, then
/// queues one job per tile on the shared worker pool. Workers write back
/// into the shared state directly; the TontooUI shell redraws
/// continuously, so no explicit invalidation is needed.
fn request_visible_tiles(shared: &Arc<Mutex<MapInner>>) {
    let now = Instant::now();
    let jobs: Vec<TileJob> = {
        let Ok(mut s) = shared.lock() else { return };
        let zoom = s.camera.zoom.round() as u8;
        let scale = (s.camera.zoom - f64::from(zoom)).exp2();
        let (vis_min_x, vis_max_x, vis_min_y, vis_max_y) =
            visible_tile_range(s.camera.center, zoom, scale, s.width, s.height);
        // Prefetch ring around the viewport (clamped to the world).
        let world_max = (1i64 << zoom) - 1;
        let (min_x, max_x) = (
            (vis_min_x - PREFETCH_MARGIN).max(0),
            (vis_max_x + PREFETCH_MARGIN).min(world_max),
        );
        let (min_y, max_y) = (
            (vis_min_y - PREFETCH_MARGIN).max(0),
            (vis_max_y + PREFETCH_MARGIN).min(world_max),
        );

        let provider = s.tile_provider();
        let cache_source = tiles::cache_source_key(provider.as_ref());
        let provider_name = provider.name().to_string();
        let snapshot = (
            s.chain.clone(),
            s.style,
            s.config.user_agent.clone(),
            s.config.timeout_seconds,
            s.config.clone(),
        );
        let (chain, style, user_agent, timeout_seconds, cache_config) = snapshot;

        let mut jobs = Vec::new();
        for ty in min_y..=max_y {
            for tx in min_x..=max_x {
                let key = TileKey::new(zoom, tx as u32, ty as u32);
                if s.tiles.contains_key(&key) || s.pending.contains(&key) {
                    continue;
                }
                // Skip recent failures; retry once the cooldown expired.
                if let Some(failed_at) = s.failed.get(&key) {
                    if now.duration_since(*failed_at) < TILE_RETRY_AFTER {
                        continue;
                    }
                    s.failed.remove(&key);
                }
                s.pending.insert(key);
                jobs.push(TileJob {
                    chain: chain.clone(),
                    style,
                    provider_name: provider_name.clone(),
                    cache_source: cache_source.clone(),
                    user_agent: user_agent.clone(),
                    timeout_seconds,
                    cache_config: cache_config.clone(),
                    key,
                    target: Arc::downgrade(shared),
                });
            }
        }

        // Center tiles first: the middle of the viewport fills in before
        // the edges, which reads as much faster loading.
        let (cfx, cfy) = tiles::tile_for(s.camera.center, zoom);
        jobs.sort_by(|a, b| {
            let da = (f64::from(a.key.x) + 0.5 - cfx).powi(2)
                + (f64::from(a.key.y) + 0.5 - cfy).powi(2);
            let db = (f64::from(b.key.x) + 0.5 - cfx).powi(2)
                + (f64::from(b.key.y) + 0.5 - cfy).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Offline badge: on while any tile inside the actual viewport
        // (not the prefetch ring) failed.
        s.offline = (vis_min_y..=vis_max_y).any(|ty| {
            (vis_min_x..=vis_max_x)
                .any(|tx| s.failed.contains_key(&TileKey::new(zoom, tx as u32, ty as u32)))
        });
        jobs
    };

    for job in jobs {
        // The pool lives for the process lifetime; send only fails when
        // all workers died, in which case the pending flag stays and the
        // next frame re-queues the tile.
        let _ = tile_pool().send(job);
    }
}

// ═══════════════════════════════════════════════════════════════
// Vello rendering (all geometry in physical px: logical * dpr)
// ═══════════════════════════════════════════════════════════════

/// Cache key for a tile inside TontooUI's [`ImageLoader`].
fn tile_image_key(provider: &str, source: &str, key: TileKey) -> String {
    format!("mapskit/{provider}/{source}/{}/{}/{}", key.z, key.x, key.y)
}

/// How many zoom levels up the parent fallback search goes. Beyond that
/// the upscaled imagery is too blurry to be worth drawing.
const OVERZOOM_PARENT_LEVELS: u8 = 4;

/// Fallback imagery for a missing tile: the nearest loaded parent tile.
///
/// Returns the parent key plus the missing tile's offset inside it
/// (`ox`, `oy` in parent sub-tiles) and the level difference. Renderers
/// draw the parent image scaled up so the sub-region covers the missing
/// tile exactly, which keeps the map flawless while zooming instead of
/// flashing placeholders.
fn parent_fallback(
    tiles: &HashMap<TileKey, Vec<u8>>,
    z: u8,
    tx: u32,
    ty: u32,
) -> Option<(TileKey, u32, u32, u32)> {
    for shift in 1..=OVERZOOM_PARENT_LEVELS {
        let Some(pz) = z.checked_sub(shift) else {
            break;
        };
        let px = tx >> shift;
        let py = ty >> shift;
        let parent = TileKey::new(pz, px, py);
        if tiles.contains_key(&parent) {
            return Some((parent, tx - (px << shift), ty - (py << shift), shift as u32));
        }
    }
    None
}

/// Draws one tile image at a logical rect.
fn draw_tile_image(
    scene: &mut Scene,
    images: &mut ImageLoader<'_>,
    cache_key: &str,
    bytes: &[u8],
    lx: f64,
    ly: f64,
    logical_size: f64,
    dpr: f64,
) {
    if let Some((image, iw, _ih)) = images.raster(cache_key, bytes, 512) {
        let s = (logical_size / iw as f64) * dpr;
        let transform = Affine::translate((lx * dpr, ly * dpr)) * Affine::scale(s);
        scene.draw_image(&image, transform);
    }
}

fn placeholder_rect(scene: &mut Scene, style: MapStyle, lx: f64, ly: f64, size: f64, dpr: f64) {
    let fill = match style {
        MapStyle::Dark | MapStyle::Satellite => Color::from_rgba8(255, 255, 255, 13),
        _ => Color::from_rgba8(0, 0, 0, 15),
    };
    let rect = vello::kurbo::Rect::new(lx * dpr, ly * dpr, (lx + size) * dpr, (ly + size) * dpr);
    scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(fill), None, &rect);
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
            if let Some(bytes) = inner.tiles.get(&key) {
                let cache_key = tile_image_key(&provider_name, &source, key);
                draw_tile_image(scene, images, &cache_key, bytes, lx, ly, tile_draw, dpr);
                continue;
            }
            // Overzoom: reuse a loaded parent tile scaled up so zooming
            // never flashes empty placeholders.
            if let Some((parent, off_x, off_y, shift)) =
                parent_fallback(&inner.tiles, z, tx as u32, ty as u32)
            {
                if let Some(bytes) = inner.tiles.get(&parent) {
                    let cache_key = tile_image_key(&provider_name, &source, parent);
                    // The parent covers 2^shift sub-tiles per axis; draw it
                    // large and clip to this tile's rect.
                    let clip = vello::kurbo::Rect::new(
                        lx * dpr,
                        ly * dpr,
                        (lx + tile_draw) * dpr,
                        (ly + tile_draw) * dpr,
                    );
                    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
                    draw_tile_image(
                        scene,
                        images,
                        &cache_key,
                        bytes,
                        lx - off_x as f64 * tile_draw,
                        ly - off_y as f64 * tile_draw,
                        tile_draw * f64::from(1u32 << shift),
                        dpr,
                    );
                    scene.pop_layer();
                    continue;
                }
            }
            // Underzoom: reuse loaded children (one level down) per quadrant
            // so zooming out also keeps imagery on screen.
            if z < tiles::MAX_ZOOM {
                let mut covered = false;
                for (qx, qy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let child = TileKey::new(z + 1, tx as u32 * 2 + qx, ty as u32 * 2 + qy);
                    if let Some(bytes) = inner.tiles.get(&child) {
                        let cache_key = tile_image_key(&provider_name, &source, child);
                        draw_tile_image(
                            scene,
                            images,
                            &cache_key,
                            bytes,
                            lx + qx as f64 * tile_draw / 2.0,
                            ly + qy as f64 * tile_draw / 2.0,
                            tile_draw / 2.0,
                            dpr,
                        );
                        covered = true;
                    }
                }
                if covered {
                    continue;
                }
            }
            placeholder_rect(scene, inner.style, lx, ly, tile_draw, dpr);
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
    let dark = matches!(inner.style, MapStyle::Dark | MapStyle::Satellite);
    // Frosted pill: light fill with dark text in light mode, dark fill
    // with white text in dark/satellite mode.
    let (fill, line, text) = if dark {
        (
            Color::from_rgba8(20, 22, 24, 200),
            Color::from_rgba8(255, 255, 255, 46),
            Color::WHITE,
        )
    } else {
        (
            Color::from_rgba8(255, 255, 255, 225),
            Color::from_rgba8(0, 0, 0, 46),
            Color::from_rgb8(0x27, 0x27, 0x27),
        )
    };
    let layout = fonts.layout_text(&label, 11.0, text, None);
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
        &Brush::Solid(fill),
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
    scene.stroke(
        &Stroke::new(1.0 * dpr),
        Affine::IDENTITY,
        &Brush::Solid(line),
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
                failed: HashMap::new(),
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
                zoom_anim: None,
                user_location: None,
                location_cache: None,
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

    /// The current camera (the interpolated one while a zoom animation
    /// is running).
    pub fn camera(&self) -> MapCamera {
        self.shared
            .lock()
            .map(|s| {
                let (center, zoom) = s.displayed_camera(Instant::now());
                MapCamera {
                    center,
                    zoom,
                    pitch: s.camera.pitch,
                    heading: s.camera.heading,
                }
            })
            .unwrap_or_default()
    }

    /// Replaces the camera and notifies the camera callback. Cancels any
    /// running zoom animation.
    pub fn set_camera(&self, camera: MapCamera) {
        if let Ok(mut s) = self.shared.lock() {
            s.camera = camera;
            s.zoom_anim = None;
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
    /// The last fix is cached for [`LOCATION_CACHE_TTL`]: while fresh, the
    /// dot appears and the camera centers immediately without querying
    /// again. Otherwise CoreLocation is queried on a worker thread; when
    /// it answers, the blue user dot appears, the camera centers on it and
    /// the fix is cached. Errors are logged via `MAPSKIT_DEBUG` only.
    pub fn show_user_location(&self) {
        // Fast path: reuse a fresh cached fix.
        let cached = {
            match self.shared.lock() {
                Ok(mut s) => {
                    if let Some((coordinate, at)) = s.location_cache {
                        if at.elapsed() < LOCATION_CACHE_TTL {
                            s.user_location = Some(coordinate);
                            s.zoom_anim = None;
                            let zoom = s.camera.zoom.max(13.0);
                            s.camera = MapCamera::new(coordinate, zoom);
                            s.on_camera_changed.clone().map(|cb| (cb, s.camera))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        };
        if let Some((cb, camera)) = cached {
            cb(&camera);
            return;
        }

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
                                s.location_cache = Some((coordinate, Instant::now()));
                                s.zoom_anim = None;
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
    /// Also refreshes the location cache, so a following
    /// `show_user_location` reuses it while fresh.
    pub fn set_user_location(&self, coordinate: Coordinate) {
        if let Ok(mut s) = self.shared.lock() {
            s.user_location = Some(coordinate);
            s.location_cache = Some((coordinate, Instant::now()));
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
            // A press takes over from a running zoom animation.
            s.cancel_anim(Instant::now());
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
                            // (viewport north). Cancels a running zoom
                            // animation first (snaps to displayed state).
                            s.cancel_anim(Instant::now());
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
        // positive); one notch (~20 px) is one zoom level. Zooms around
        // the viewport center so the image stays put. The step is
        // animated (~200 ms ease-out); the camera callback fires when the
        // animation completes.
        let delta = (-dy / 20.0).clamp(-3.0, 3.0);
        if delta == 0.0 {
            return;
        }
        if let Ok(mut s) = self.shared.lock() {
            let (cx, cy) = (s.width / 2.0, s.height / 2.0);
            s.begin_zoom_step(cx, cy, delta);
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
        // Advance a running zoom animation before requesting tiles so the
        // new frame already fetches for the interpolated camera.
        let completed = match self.shared.lock() {
            Ok(mut s) => s.advance_anim(Instant::now()),
            Err(_) => None,
        };
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
        drop(inner);

        // Fire the camera callback once the zoom animation lands.
        if let Some((cb, camera)) = completed {
            cb(&camera);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_fallback_finds_nearest_loaded_parent() {
        let mut tiles = HashMap::new();
        // Missing tile 10/8/4: parent at z9 is (4, 2), grandparent at z8
        // is (2, 1).
        tiles.insert(TileKey::new(8, 2, 1), vec![1]);
        tiles.insert(TileKey::new(9, 4, 2), vec![2]);
        // Nearest parent wins with the sub-tile offset inside it.
        assert_eq!(
            parent_fallback(&tiles, 10, 8, 4),
            Some((TileKey::new(9, 4, 2), 0, 0, 1))
        );
        // Odd tile coords produce the offset: 10/9/5 sits at (1, 1) inside
        // parent 9/4/2.
        assert_eq!(
            parent_fallback(&tiles, 10, 9, 5),
            Some((TileKey::new(9, 4, 2), 1, 1, 1))
        );
        // Without the z9 parent the z8 grandparent is used.
        tiles.remove(&TileKey::new(9, 4, 2));
        assert_eq!(
            parent_fallback(&tiles, 10, 9, 5),
            Some((TileKey::new(8, 2, 1), 1, 1, 2))
        );
    }

    #[test]
    fn parent_fallback_respects_depth_and_emptiness() {
        let mut tiles = HashMap::new();
        // Only a z0 world tile: reachable within 4 levels from z4.
        tiles.insert(TileKey::new(0, 0, 0), vec![1]);
        assert_eq!(
            parent_fallback(&tiles, 4, 9, 5),
            Some((TileKey::new(0, 0, 0), 9, 5, 4))
        );
        // Too deep: z5 is 5 levels above z0.
        assert_eq!(parent_fallback(&tiles, 5, 20, 11), None);
        // Empty cache: no fallback at all.
        assert_eq!(parent_fallback(&HashMap::new(), 10, 8, 4), None);
    }

    #[test]
    fn cached_location_centers_immediately() {
        // A fresh manually set location must be reused synchronously, with
        // no CoreLocation daemon involved.
        let map = MapView::new(&MapsConfiguration::new());
        let berlin = Coordinate::new(52.52, 13.405);
        map.set_user_location(berlin);
        map.set_center(Coordinate::new(48.137, 11.575), 10.0);
        map.show_user_location();
        let camera = map.camera();
        assert!((camera.center.latitude - berlin.latitude).abs() < 1e-9);
        assert!((camera.center.longitude - berlin.longitude).abs() < 1e-9);
        assert!(camera.zoom >= 13.0);
        assert_eq!(map.user_location(), Some(berlin));
    }

    #[test]
    fn stale_cache_queries_again() {
        // An expired cache entry must not be reused: with no daemon
        // reachable the camera stays where it is.
        let map = MapView::new(&MapsConfiguration::new());
        {
            let mut s = map.shared.lock().unwrap();
            s.location_cache = Some((
                Coordinate::new(52.52, 13.405),
                Instant::now() - LOCATION_CACHE_TTL - Duration::from_secs(1),
            ));
        }
        map.set_center(Coordinate::new(48.137, 11.575), 10.0);
        map.show_user_location();
        // Give the worker thread no chance: the fast path would have
        // centered synchronously.
        let camera = map.camera();
        assert!((camera.center.latitude - 48.137).abs() < 1e-9);
    }

    #[test]
    fn ease_out_cubic_shape() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // Fast start: halfway through time covers most of the distance.
        assert!(ease_out_cubic(0.5) > 0.5);
        assert!(ease_out_cubic(0.5) < 1.0);
    }

    #[test]
    fn lerp_takes_shortest_longitude_path() {
        // 179 to -179 crosses the antimeridian (+2 degrees), not the long
        // way around (-358 degrees).
        let mid = lerp_coord(Coordinate::new(0.0, 179.0), Coordinate::new(0.0, -179.0), 0.5);
        assert!((mid.longitude.abs() - 180.0).abs() < 1e-9);
        let end = lerp_coord(Coordinate::new(10.0, 179.0), Coordinate::new(20.0, -179.0), 1.0);
        assert!((end.latitude - 20.0).abs() < 1e-9);
        assert!((end.longitude + 179.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_step_animates_to_target() {
        let map = MapView::new(&MapsConfiguration::new());
        map.set_center(Coordinate::new(52.52, 13.405), 10.0);
        {
            let mut s = map.shared.lock().unwrap();
            s.width = 800.0;
            s.height = 600.0;
            s.begin_zoom_step(400.0, 300.0, 1.0);
            assert!(s.zoom_anim.is_some());
            let target_zoom = s.zoom_anim.as_ref().unwrap().to_zoom;
            assert!((target_zoom - 11.0).abs() < 1e-9);
            // Mid-flight the displayed camera is between start and target.
            let anim_start = s.zoom_anim.as_ref().unwrap().start;
            let (mid_center, mid_zoom) = s.displayed_camera(anim_start + ZOOM_ANIM_DURATION / 2);
            assert!(mid_zoom > 10.0 && mid_zoom < 11.0);
            let _ = mid_center;
            // Completing the animation lands exactly on target and fires.
            let done = s.advance_anim(anim_start + ZOOM_ANIM_DURATION + Duration::from_millis(1));
            assert!(s.zoom_anim.is_none());
            assert!((s.camera.zoom - 11.0).abs() < 1e-9);
            // No callback registered, so nothing to fire.
            assert!(done.is_none());
        }
    }

    #[test]
    fn zoom_step_retargets_mid_flight() {
        let map = MapView::new(&MapsConfiguration::new());
        map.set_center(Coordinate::new(52.52, 13.405), 10.0);
        {
            let mut s = map.shared.lock().unwrap();
            s.width = 800.0;
            s.height = 600.0;
            s.begin_zoom_step(400.0, 300.0, 1.0);
            let start = s.zoom_anim.as_ref().unwrap().start;
            // Second notch before the first lands: retargets from the
            // displayed state toward zoom 12-ish, single animation.
            s.begin_zoom_step(400.0, 300.0, 1.0);
            assert!(s.zoom_anim.is_some());
            let _ = start;
            let target = s.zoom_anim.as_ref().unwrap().to_zoom;
            assert!(target > 11.0 && target <= 12.0 + 1e-9);
        }
    }
}
