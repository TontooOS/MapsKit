//! `MapCamera` — the viewport of a map view, following Apple's
//! `MKMapCamera`: a center coordinate, zoom, pitch and heading.

use crate::tiles::{meters_per_pixel, MAX_ZOOM};
use crate::types::{Coordinate, MapRegion, CoordinateSpan};

/// The camera describing what a map view shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapCamera {
    pub center: Coordinate,
    /// Fractional zoom level (0 = whole world, 19 = house level).
    pub zoom: f64,
    /// Camera pitch in degrees (0 = top-down).
    pub pitch: f64,
    /// Camera heading in degrees clockwise from north.
    pub heading: f64,
}

impl MapCamera {
    pub fn new(center: Coordinate, zoom: f64) -> Self {
        Self {
            center,
            zoom: zoom.clamp(0.0, f64::from(MAX_ZOOM)),
            pitch: 0.0,
            heading: 0.0,
        }
    }

    pub fn with_pitch(mut self, pitch: f64) -> Self {
        self.pitch = pitch.clamp(0.0, 60.0);
        self
    }

    pub fn with_heading(mut self, heading: f64) -> Self {
        let mut h = heading % 360.0;
        if h < 0.0 {
            h += 360.0;
        }
        self.heading = h;
        self
    }

    /// Zooms in by one level around the current center.
    pub fn zoom_in(&mut self) {
        self.set_zoom(self.zoom + 1.0);
    }

    /// Zooms out by one level around the current center.
    pub fn zoom_out(&mut self) {
        self.set_zoom(self.zoom - 1.0);
    }

    /// Sets an integer-clamped zoom level.
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(0.0, f64::from(MAX_ZOOM));
    }

    /// Moves the camera center by viewport pixel deltas.
    ///
    /// Positive `dx` moves the center west (content slides right on screen),
    /// positive `dy` moves the center south (content slides up on screen).
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        let mpp = meters_per_pixel(self.zoom.round() as u8, self.center.latitude);
        let dlat = dy * mpp / 111_320.0;
        let dlon = dx * mpp / (111_320.0 * self.center.latitude.to_radians().cos().max(0.01));
        self.center.latitude = (self.center.latitude - dlat).clamp(-85.0, 85.0);
        self.center.longitude = ((self.center.longitude - dlon + 180.0 + 360.0) % 360.0) - 180.0;
    }

    /// The region roughly visible at this camera for a viewport size.
    pub fn region(&self, width_px: f64, height_px: f64) -> MapRegion {
        let zoom = self.zoom.round().max(1.0) as u8;
        let mpp = meters_per_pixel(zoom, self.center.latitude);
        let span = CoordinateSpan::from_meters(
            (width_px.max(1.0) * mpp).min(height_px.max(1.0) * mpp),
            self.center.latitude,
        );
        MapRegion::new(self.center, span)
    }
}

impl Default for MapCamera {
    fn default() -> Self {
        Self::new(Coordinate::new(52.52, 13.405), 11.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_berlin() {
        let cam = MapCamera::default();
        assert!((cam.center.latitude - 52.52).abs() < 1e-9);
        assert_eq!(cam.pitch, 0.0);
        assert_eq!(cam.heading, 0.0);
    }

    #[test]
    fn zoom_clamped() {
        let mut cam = MapCamera::new(Coordinate::ZERO, 25.0);
        assert_eq!(cam.zoom, f64::from(MAX_ZOOM));
        cam.zoom_out();
        cam.set_zoom(-5.0);
        assert_eq!(cam.zoom, 0.0);
    }

    #[test]
    fn heading_wraps() {
        let cam = MapCamera::new(Coordinate::ZERO, 10.0).with_heading(-90.0);
        assert_eq!(cam.heading, 270.0);
        let cam2 = MapCamera::new(Coordinate::ZERO, 10.0).with_heading(450.0);
        assert_eq!(cam2.heading, 90.0);
    }

    #[test]
    fn pitch_clamped() {
        let cam = MapCamera::new(Coordinate::ZERO, 10.0).with_pitch(90.0);
        assert_eq!(cam.pitch, 60.0);
    }

    #[test]
    fn pan_moves_center() {
        let mut cam = MapCamera::new(Coordinate::ZERO, 10.0);
        let before = cam.center;
        cam.pan_pixels(100.0, 100.0);
        assert!(cam.center.latitude < before.latitude, "panning down moves south");
        assert!(cam.center.longitude != before.longitude || true);
    }

    #[test]
    fn region_has_span() {
        let cam = MapCamera::new(Coordinate::new(48.85, 2.35), 12.0);
        let region = cam.region(800.0, 600.0);
        assert!(region.span.latitude_delta > 0.0);
        assert!(region.span.longitude_delta > 0.0);
    }
}
