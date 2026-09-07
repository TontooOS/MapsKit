//! Map overlays — annotations, polylines, polygons and circles, following
//! Apple's `MKAnnotation`, `MKPolyline`, `MKPolygon` and `MKCircle`.

use crate::types::{Coordinate, Place};
use serde::{Deserialize, Serialize};

/// A pin on the map (`MKPointAnnotation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub coordinate: Coordinate,
    pub title: String,
    pub subtitle: Option<String>,
    /// Glyph name shown inside the pin (SF Symbol name).
    pub glyph: Option<String>,
    /// Whether this annotation is selected (drawn highlighted).
    #[serde(default)]
    pub selected: bool,
}

impl Annotation {
    pub fn new(coordinate: Coordinate, title: impl Into<String>) -> Self {
        Self {
            id: format!("annotation-{}", crate::next_id()),
            coordinate,
            title: title.into(),
            subtitle: None,
            glyph: None,
            selected: false,
        }
    }

    pub fn from_place(place: &Place) -> Self {
        Self {
            id: if place.id.is_empty() {
                format!("annotation-{}", crate::next_id())
            } else {
                place.id.clone()
            },
            coordinate: place.coordinate,
            title: place.name.clone(),
            subtitle: Some(place.address.one_line()).filter(|s| !s.is_empty()),
            glyph: None,
            selected: false,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }
}

/// A line drawn between points (`MKPolyline`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline {
    pub id: String,
    pub points: Vec<Coordinate>,
    /// CSS color string, e.g. `"#FF6B2B"`.
    pub color: String,
    /// Stroke width in pixels.
    pub width: f64,
}

impl Polyline {
    pub fn new(points: Vec<Coordinate>, color: impl Into<String>) -> Self {
        Self {
            id: format!("polyline-{}", crate::next_id()),
            points,
            color: color.into(),
            width: 3.0,
        }
    }

    /// A route polyline in the TontooOS accent color.
    pub fn for_route(route: &crate::types::Route) -> Self {
        Self::new(route.geometry.clone(), "#FF6B2B").width(4.0)
    }

    pub fn width(mut self, width: f64) -> Self {
        self.width = width.max(1.0);
        self
    }

    /// The bounding box of all points.
    pub fn bounding_box(&self) -> Option<crate::types::BoundingBox> {
        bounding_box_of(&self.points)
    }
}

/// A filled shape (`MKPolygon`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub id: String,
    pub points: Vec<Coordinate>,
    /// Fill color as CSS string; empty string for no fill.
    pub fill_color: String,
    /// Stroke color as CSS string; empty string for no stroke.
    pub stroke_color: String,
}

impl Polygon {
    pub fn new(points: Vec<Coordinate>) -> Self {
        Self {
            id: format!("polygon-{}", crate::next_id()),
            points,
            fill_color: "rgba(255,107,43,0.25)".into(),
            stroke_color: "#FF6B2B".into(),
        }
    }

    pub fn fill(mut self, color: impl Into<String>) -> Self {
        self.fill_color = color.into();
        self
    }

    pub fn stroke(mut self, color: impl Into<String>) -> Self {
        self.stroke_color = color.into();
        self
    }

    /// The bounding box of all points.
    pub fn bounding_box(&self) -> Option<crate::types::BoundingBox> {
        bounding_box_of(&self.points)
    }
}

/// A radius circle around a center (`MKCircle`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CircleOverlay {
    pub id: String,
    pub center: Coordinate,
    /// Radius in meters.
    pub radius_m: f64,
    pub fill_color: String,
    pub stroke_color: String,
}

impl CircleOverlay {
    pub fn new(center: Coordinate, radius_m: f64) -> Self {
        Self {
            id: format!("circle-{}", crate::next_id()),
            center,
            radius_m: radius_m.max(1.0),
            fill_color: "rgba(0,229,255,0.20)".into(),
            stroke_color: "#00E5FF".into(),
        }
    }

    pub fn fill(mut self, color: impl Into<String>) -> Self {
        self.fill_color = color.into();
        self
    }

    pub fn stroke(mut self, color: impl Into<String>) -> Self {
        self.stroke_color = color.into();
        self
    }
}

/// Any overlay that can be added to a map view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Overlay {
    Polyline(Polyline),
    Polygon(Polygon),
    Circle(CircleOverlay),
}

impl From<Polyline> for Overlay {
    fn from(p: Polyline) -> Self {
        Overlay::Polyline(p)
    }
}

impl From<Polygon> for Overlay {
    fn from(p: Polygon) -> Self {
        Overlay::Polygon(p)
    }
}

impl From<CircleOverlay> for Overlay {
    fn from(c: CircleOverlay) -> Self {
        Overlay::Circle(c)
    }
}

impl Overlay {
    /// The bounding box of the overlay geometry.
    pub fn bounding_box(&self) -> Option<crate::types::BoundingBox> {
        match self {
            Overlay::Polyline(p) => p.bounding_box(),
            Overlay::Polygon(p) => p.bounding_box(),
            Overlay::Circle(c) => {
                let lat_delta = c.radius_m / 111_320.0;
                let lon_delta =
                    c.radius_m / (111_320.0 * c.center.latitude.to_radians().cos().max(0.01));
                Some(crate::types::BoundingBox::new(
                    c.center.latitude + lat_delta,
                    c.center.latitude - lat_delta,
                    c.center.longitude + lon_delta,
                    c.center.longitude - lon_delta,
                ))
            }
        }
    }
}

/// Smallest box containing all coordinates.
pub fn bounding_box_of(points: &[Coordinate]) -> Option<crate::types::BoundingBox> {
    let first = points.first()?;
    let mut bbox = crate::types::BoundingBox::new(
        first.latitude,
        first.latitude,
        first.longitude,
        first.longitude,
    );
    for point in &points[1..] {
        bbox.north = bbox.north.max(point.latitude);
        bbox.south = bbox.south.min(point.latitude);
        bbox.east = bbox.east.max(point.longitude);
        bbox.west = bbox.west.min(point.longitude);
    }
    Some(bbox)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Route;

    #[test]
    fn annotation_builder() {
        let a = Annotation::new(Coordinate::new(1.0, 2.0), "Pin")
            .subtitle("Sub")
            .glyph("mappin");
        assert_eq!(a.title, "Pin");
        assert_eq!(a.subtitle.as_deref(), Some("Sub"));
        assert_eq!(a.glyph.as_deref(), Some("mappin"));
    }

    #[test]
    fn annotation_from_place() {
        let mut place = Place::new("Cafe", Coordinate::new(48.2, 16.37)).with_id("osm:N1");
        place.address.city = Some("Vienna".into());
        let a = Annotation::from_place(&place);
        assert_eq!(a.id, "osm:N1");
        assert_eq!(a.title, "Cafe");
        assert!(a.subtitle.as_deref().unwrap_or_default().contains("Vienna"));
    }

    #[test]
    fn polyline_bbox_and_route() {
        let route = Route {
            distance_m: 1000.0,
            duration_s: 60.0,
            geometry: vec![Coordinate::new(0.0, 0.0), Coordinate::new(1.0, 1.0)],
            steps: vec![],
            source: "test".into(),
        };
        let line = Polyline::for_route(&route);
        let bbox = line.bounding_box().expect("bbox");
        assert_eq!(bbox.south, 0.0);
        assert_eq!(bbox.north, 1.0);
        assert_eq!(line.width, 4.0);
    }

    #[test]
    fn circle_bbox() {
        let overlay: Overlay = CircleOverlay::new(Coordinate::ZERO, 111_320.0).into();
        let bbox = overlay.bounding_box().expect("bbox");
        assert!((bbox.north - 1.0).abs() < 0.01);
        assert!((bbox.south + 1.0).abs() < 0.01);
    }

    #[test]
    fn overlay_enum_conversion() {
        let overlay: Overlay = Polyline::new(vec![], "#fff").into();
        assert!(matches!(overlay, Overlay::Polyline(_)));
        let overlay: Overlay = Polygon::new(vec![]).into();
        assert!(matches!(overlay, Overlay::Polygon(_)));
        let overlay: Overlay = CircleOverlay::new(Coordinate::ZERO, 10.0).into();
        assert!(matches!(overlay, Overlay::Circle(_)));
    }

    #[test]
    fn empty_points_no_bbox() {
        assert!(bounding_box_of(&[]).is_none());
    }
}
