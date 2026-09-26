//! `GlobeView` — the optional 3D-feel globe view, following Apple's
//! `MKMapView` globe elevation mode.
//!
//! The earth is rendered as a shaded disc with Vello inside TontooUI: an
//! ocean gradient, a graticule every 15 degrees and accent colored marker
//! dots on the front hemisphere. No OpenGL context is required, so the
//! globe works on every TontooUI target (including WGPU-only systems).
//!
//! Interaction: drag rotates the globe, the scroll wheel zooms, and an
//! optional auto-rotate mode slowly spins the earth. Markers are drawn as
//! accent colored dots on the front hemisphere.
//!
//! ```rust,no_run
//! use tontooui::elements::View;
//! use mapskit::prelude::*;
//!
//! let mut globe = GlobeView::new(&MapsConfiguration::new().style(MapStyle::Dark));
//! globe.set_center(Coordinate::new(48.137, 11.575));
//! globe.add_marker(GlobeMarker::new(Coordinate::new(48.137, 11.575), "Munich"));
//! globe.set_auto_rotate(true);
//! ```

use crate::config::MapsConfiguration;
use crate::types::Coordinate;

use std::any::Any;
use std::sync::{Arc, Mutex};

use tontooui::elements::layout::View as TontooView;
use tontooui::renderer::images::ImageLoader;
use tontooui::renderer::text::{FontSystem, draw_layout};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Circle as KurboCircle, Ellipse, Stroke};
use vello::peniko::{Brush, Color, Fill};

/// A dot marker on the globe.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobeMarker {
    pub coordinate: Coordinate,
    pub label: String,
}

impl GlobeMarker {
    pub fn new(coordinate: Coordinate, label: impl Into<String>) -> Self {
        Self {
            coordinate,
            label: label.into(),
        }
    }
}

const MAX_MARKERS: usize = 64;

/// Closest camera distance (surface nearly fills the view).
const MIN_DISTANCE: f64 = 1.15;
/// Farthest camera distance (whole globe visible).
const MAX_DISTANCE: f64 = 6.0;

/// Degrees of rotation per frame at ~60 fps while auto-rotating
/// (~11 deg/s, matching the previous GL timer cadence).
const AUTO_ROTATE_STEP: f64 = 0.18;
const ROTATION_DEGREES_PER_PIXEL: f64 = 0.25;

struct GlobeInner {
    #[allow(dead_code)]
    config: MapsConfiguration,
    center_lat: f64,
    center_lon: f64,
    distance: f64,
    auto_rotate: bool,
    dragging: bool,
    last_hover: Option<(f64, f64)>,
    markers: Vec<GlobeMarker>,
}

impl GlobeInner {
    /// Clamps latitude, wraps longitude and bounds the camera distance.
    fn clamp_center(&mut self) {
        self.center_lat = self.center_lat.clamp(-89.0, 89.0);
        if !(-180.0..=180.0).contains(&self.center_lon) {
            self.center_lon = ((self.center_lon + 180.0).rem_euclid(360.0)) - 180.0;
        }
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Orthographic projection of a lat/lon point onto the disc.
    ///
    /// Returns `(x, y, visible)` in unit-disc space (`-1..=1`), where
    /// `visible` is true on the front hemisphere facing the viewer.
    fn project(&self, latitude: f64, longitude: f64) -> (f64, f64, bool) {
        let phi = latitude.to_radians();
        let lambda = longitude.to_radians();
        let (x0, y0, z0) = (
            phi.cos() * lambda.sin(),
            phi.sin(),
            phi.cos() * lambda.cos(),
        );
        // Rotate the world so the center coordinate faces the viewer
        // (+Z): first yaw by -center_lon around Y, then pitch by
        // center_lat around X.
        let lon = (-self.center_lon).to_radians();
        let (x1, z1) = (x0 * lon.cos() + z0 * lon.sin(), -x0 * lon.sin() + z0 * lon.cos());
        let lat = self.center_lat.to_radians();
        let (y2, z2) = (y0 * lat.cos() - z1 * lat.sin(), y0 * lat.sin() + z1 * lat.cos());
        (x1, y2, z2 > 0.10)
    }
}

// ═══════════════════════════════════════════════════════════════
// GlobeView
// ═══════════════════════════════════════════════════════════════

/// The interactive globe view for TontooUI.
///
/// Implements [`TontooView`], so it embeds into any stack. State is shared
/// through an `Arc<Mutex<..>>`; the view is `Clone` (clones share the same
/// globe) and all state-changing methods take `&self`.
///
/// Host `App`s must forward `mouse_down` / `mouse_up` / `mouse_wheel` and
/// `mouse_move` (as `set_hover`) for rotation and zoom to work.
#[derive(Clone)]
pub struct GlobeView {
    shared: Arc<Mutex<GlobeInner>>,
    x: f32,
    y: f32,
    placed_w: f32,
    placed_h: f32,
}

impl GlobeView {
    /// Creates a globe view.
    ///
    /// `config.globe_earth_texture` is accepted for compatibility but
    /// currently reserved: this renderer draws a procedural ocean with a
    /// graticule and needs no downloaded imagery.
    pub fn new(config: &MapsConfiguration) -> Self {
        let _ = config.globe_earth_texture;
        Self {
            shared: Arc::new(Mutex::new(GlobeInner {
                config: config.clone(),
                center_lat: 25.0,
                center_lon: 10.0,
                distance: 3.0,
                auto_rotate: true,
                dragging: false,
                last_hover: None,
                markers: Vec::new(),
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

    // ─── Camera ────────────────────────────────────────────

    /// Centers the globe on a coordinate.
    pub fn set_center(&self, coordinate: Coordinate) {
        if let Ok(mut s) = self.shared.lock() {
            s.center_lat = coordinate.latitude;
            s.center_lon = coordinate.longitude;
            s.clamp_center();
        }
    }

    /// The coordinate currently facing the viewer.
    pub fn center(&self) -> Coordinate {
        self.shared
            .lock()
            .map(|s| Coordinate::new(s.center_lat, s.center_lon))
            .unwrap_or(Coordinate::new(25.0, 10.0))
    }

    /// Sets the camera distance (smaller = closer).
    pub fn set_distance(&self, distance: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.distance = distance;
            s.clamp_center();
        }
    }

    /// The camera distance.
    pub fn distance(&self) -> f64 {
        self.shared.lock().map(|s| s.distance).unwrap_or(3.0)
    }

    // ─── Rotation ──────────────────────────────────────────

    /// Enables or disables the idle spin.
    pub fn set_auto_rotate(&self, enabled: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.auto_rotate = enabled;
        }
    }

    /// Whether idle spin is enabled.
    pub fn auto_rotate(&self) -> bool {
        self.shared.lock().map(|s| s.auto_rotate).unwrap_or(false)
    }

    // ─── Markers ───────────────────────────────────────────

    /// Replaces all markers.
    pub fn set_markers(&self, markers: Vec<GlobeMarker>) {
        if let Ok(mut s) = self.shared.lock() {
            s.markers = markers.into_iter().take(MAX_MARKERS).collect();
        }
    }

    /// Adds one marker. At most [`MAX_MARKERS`] markers are kept.
    pub fn add_marker(&self, marker: GlobeMarker) {
        if let Ok(mut s) = self.shared.lock() {
            if s.markers.len() < MAX_MARKERS {
                s.markers.push(marker);
            }
        }
    }

    /// Removes all markers.
    pub fn clear_markers(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.markers.clear();
        }
    }

    /// All current markers.
    pub fn markers(&self) -> Vec<GlobeMarker> {
        self.shared.lock().map(|s| s.markers.clone()).unwrap_or_default()
    }

    // ─── Texture ───────────────────────────────────────────

    /// Starts fetching a fresh earth texture from the providers.
    ///
    /// Currently a compatibility no-op: the Vello renderer draws a
    /// procedural ocean and needs no downloaded imagery.
    pub fn reload_earth_texture(&self) {}
}

impl TontooView for GlobeView {
    fn measure(&mut self, _fonts: &mut FontSystem) -> (f32, f32) {
        (self.placed_w.max(200.0), self.placed_h.max(150.0))
    }

    fn place(&mut self, _fonts: &mut FontSystem, x: f32, y: f32, width: f32, height: f32) {
        self.x = x;
        self.y = y;
        self.placed_w = width.max(1.0);
        self.placed_h = height.max(1.0);
    }

    fn draw(&mut self, scene: &mut Scene, fonts: &mut FontSystem, _images: &mut ImageLoader<'_>) {
        if self.placed_w <= 0.0 || self.placed_h <= 0.0 {
            return;
        }
        let dpr = fonts.scale as f64;
        let (ox, oy) = (self.x as f64, self.y as f64);

        let mut inner = match self.shared.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        // Idle spin advances one step per frame (~60 fps).
        if inner.auto_rotate && !inner.dragging {
            inner.center_lon -= AUTO_ROTATE_STEP;
            inner.clamp_center();
        }

        // Disc geometry: fit the globe into the view with a margin that
        // shrinks as the camera zooms in.
        let zoom_t = ((MAX_DISTANCE - inner.distance) / (MAX_DISTANCE - MIN_DISTANCE)).clamp(0.0, 1.0);
        let radius = (self.placed_w.min(self.placed_h) as f64 / 2.0 - 8.0).max(10.0)
            * (0.55 + 0.45 * zoom_t);
        let (cx, cy) = (ox + self.placed_w as f64 / 2.0, oy + self.placed_h as f64 / 2.0);
        let (pcx, pcy, pr) = (cx * dpr, cy * dpr, radius * dpr);

        // Ocean disc: vertical gradient bands (polar caps lighter).
        let disc = KurboCircle::new((pcx, pcy), pr);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgb8(0x0a, 0x36, 0x66)),
            None,
            &disc,
        );
        let cap_n = Ellipse::new((pcx, pcy - pr * 0.82), (pr * 0.55, pr * 0.18), 0.0);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(224, 235, 245, 220)),
            None,
            &cap_n,
        );
        let cap_s = Ellipse::new((pcx, pcy + pr * 0.82), (pr * 0.55, pr * 0.18), 0.0);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(224, 235, 245, 220)),
            None,
            &cap_s,
        );

        // Terminator shading on the night side.
        let shade = KurboCircle::new((pcx + pr * 0.45, pcy - pr * 0.25), pr * 0.95);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(2, 8, 20, 90)),
            None,
            &shade,
        );
        // Re-punch the ocean so the shade only darkens the disc.
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(10, 54, 102, 110)),
            None,
            &disc,
        );

        let graticule = Color::from_rgba8(255, 255, 255, 40);
        let stroke = Stroke::new((1.0 * dpr).max(1.0));

        // Parallels every 15 degrees.
        for lat_deg in (-75..=75).step_by(15) {
            let lat = lat_deg as f64;
            let r = lat.to_radians().cos();
            if r <= 0.0 {
                continue;
            }
            // Project the parallel's center point to find its screen offset.
            let (_, py, _) = inner.project(lat, inner.center_lon);
            let ellipse = Ellipse::new((pcx, pcy + py * pr), (r * pr, (r * 0.28 * pr).max(1.0)), 0.0);
            scene.stroke(&stroke, Affine::IDENTITY, &Brush::Solid(graticule), None, &ellipse);
        }
        // Meridians every 15 degrees (front arcs only).
        for lon_deg in (0..360).step_by(15) {
            let mut path = BezPath::new();
            let mut started = false;
            for lat_deg in (-90..=90).step_by(3) {
                let (px, py, visible) = inner.project(lat_deg as f64, lon_deg as f64);
                if !visible {
                    started = false;
                    continue;
                }
                let (sx, sy) = (pcx + px * pr, pcy + py * pr);
                if started {
                    path.line_to((sx, sy));
                } else {
                    path.move_to((sx, sy));
                    started = true;
                }
            }
            if !path.elements().is_empty() {
                scene.stroke(&stroke, Affine::IDENTITY, &Brush::Solid(graticule), None, &path);
            }
        }
        // Equator emphasis.
        {
            let (_, py, _) = inner.project(0.0, inner.center_lon);
            let equator = Ellipse::new((pcx, pcy + py * pr), (pr, (0.28 * pr).max(1.0)), 0.0);
            scene.stroke(
                &stroke,
                Affine::IDENTITY,
                &Brush::Solid(Color::from_rgba8(255, 255, 255, 70)),
                None,
                &equator,
            );
        }

        // Rim light.
        let rim = KurboCircle::new((pcx, pcy), pr);
        scene.stroke(
            &Stroke::new((2.0 * dpr).max(1.0)),
            Affine::IDENTITY,
            &Brush::Solid(Color::from_rgba8(120, 180, 255, 110)),
            None,
            &rim,
        );

        // Markers on the front hemisphere.
        let markers = inner.markers.clone();
        let dot = Color::from_rgb8(0xff, 0x6b, 0x2b);
        for marker in markers.iter().take(MAX_MARKERS) {
            let (px, py, visible) = inner.project(marker.coordinate.latitude, marker.coordinate.longitude);
            if !visible {
                continue;
            }
            let (sx, sy) = (pcx + px * pr, pcy + py * pr);
            let halo = KurboCircle::new((sx, sy), 7.0 * dpr);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                &Brush::Solid(Color::from_rgba8(255, 107, 43, 70)),
                None,
                &halo,
            );
            let core = KurboCircle::new((sx, sy), 4.0 * dpr);
            scene.fill(Fill::NonZero, Affine::IDENTITY, &Brush::Solid(dot), None, &core);
            if !marker.label.is_empty() {
                let layout = fonts.layout_text(&marker.label, 11.0, Color::WHITE, None);
                draw_layout(scene, &layout, (sx / dpr + 7.0) as f32, (sy / dpr - 7.0) as f32, fonts.scale);
            }
        }
    }

    fn mouse_down(&mut self, _x: f64, _y: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.dragging = true;
            s.last_hover = Some((_x, _y));
        }
    }

    fn mouse_up(&mut self, _x: f64, _y: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.dragging = false;
            s.last_hover = None;
        }
    }

    fn set_hover(&mut self, x: f32, y: f32) {
        if let Ok(mut s) = self.shared.lock() {
            if s.dragging {
                if let Some((px, py)) = s.last_hover {
                    let (dx, dy) = (x as f64 - px, y as f64 - py);
                    s.center_lon -= dx * ROTATION_DEGREES_PER_PIXEL;
                    s.center_lat += dy * ROTATION_DEGREES_PER_PIXEL;
                    s.clamp_center();
                }
                s.last_hover = Some((x as f64, y as f64));
            }
        }
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        let _ = dx;
        let delta = (-dy / 20.0).clamp(-3.0, 3.0) * 0.25;
        if delta == 0.0 {
            return;
        }
        if let Ok(mut s) = self.shared.lock() {
            s.distance = (s.distance - delta).clamp(MIN_DISTANCE, MAX_DISTANCE);
        }
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

    fn test_view(lat: f64, lon: f64) -> GlobeView {
        let view = GlobeView::new(&MapsConfiguration::new());
        view.set_center(Coordinate::new(lat, lon));
        view
    }

    #[test]
    fn center_projection_faces_viewer() {
        for (lat, lon) in [(0.0, 0.0), (45.0, 90.0), (-30.0, -120.0)] {
            let view = test_view(lat, lon);
            let inner = view.shared.lock().unwrap();
            let (x, y, visible) = inner.project(lat, lon);
            assert!(visible, "lat={lat} lon={lon}");
            assert!(x.abs() < 0.05, "lat={lat} lon={lon}: x={x}");
            assert!(y.abs() < 0.05, "lat={lat} lon={lon}: y={y}");
        }
    }

    #[test]
    fn back_side_hidden() {
        let view = test_view(0.0, 0.0);
        let inner = view.shared.lock().unwrap();
        let (_, _, visible) = inner.project(0.0, 180.0);
        assert!(!visible);
    }

    #[test]
    fn clamp_state() {
        let view = GlobeView::new(&MapsConfiguration::new());
        view.set_center(Coordinate::new(120.0, 400.0));
        view.set_distance(50.0);
        let inner = view.shared.lock().unwrap();
        assert_eq!(inner.center_lat, 89.0);
        assert!((-180.0..=180.0).contains(&inner.center_lon));
        assert!(inner.distance <= MAX_DISTANCE);
    }

    #[test]
    fn markers_capped() {
        let view = GlobeView::new(&MapsConfiguration::new());
        for i in 0..100 {
            view.add_marker(GlobeMarker::new(Coordinate::new(0.0, 0.0), format!("m{i}")));
        }
        assert_eq!(view.markers().len(), MAX_MARKERS);
        view.clear_markers();
        assert!(view.markers().is_empty());
    }
}
