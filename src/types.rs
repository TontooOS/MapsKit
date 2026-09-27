//! Core value types shared across MapsKit.
//!
//! These mirror Apple's MapKit model types: `Coordinate` corresponds to
//! `CLLocationCoordinate2D`, `MapRegion` to `MKCoordinateRegion`, `Place` to
//! `MKMapItem` and `Route` / `RouteStep` to `MKRoute` / `MKRouteStep`.

use std::fmt;

/// A geographic coordinate in WGS 84 (degrees).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub latitude: f64,
    pub longitude: f64,
}

impl Coordinate {
    pub const ZERO: Self = Self {
        latitude: 0.0,
        longitude: 0.0,
    };

    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
        }
    }

    /// Great-circle distance to another coordinate in meters (Haversine).
    pub fn distance_to(&self, other: &Coordinate) -> f64 {
        let r = 6_371_000.0;
        let lat1 = self.latitude.to_radians();
        let lat2 = other.latitude.to_radians();
        let dlat = (other.latitude - self.latitude).to_radians();
        let dlon = (other.longitude - self.longitude).to_radians();
        let a = (dlat / 2.0).sin() * (dlat / 2.0).sin()
            + lat1.cos() * lat2.cos() * (dlon / 2.0).sin() * (dlon / 2.0).sin();
        r * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
    }

    /// Returns true when the coordinate is inside the given bounding box.
    pub fn inside(&self, bbox: &BoundingBox) -> bool {
        self.latitude >= bbox.south
            && self.latitude <= bbox.north
            && self.longitude >= bbox.west
            && self.longitude <= bbox.east
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}, {:.6}", self.latitude, self.longitude)
    }
}

/// A rectangular map region defined by a center and a lat/lon span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapRegion {
    pub center: Coordinate,
    pub span: CoordinateSpan,
}

impl MapRegion {
    pub fn new(center: Coordinate, span: CoordinateSpan) -> Self {
        Self { center, span }
    }

    /// The bounding box of this region.
    pub fn bounding_box(&self) -> BoundingBox {
        BoundingBox {
            north: self.center.latitude + self.span.latitude_delta / 2.0,
            south: self.center.latitude - self.span.latitude_delta / 2.0,
            east: self.center.longitude + self.span.longitude_delta / 2.0,
            west: self.center.longitude - self.span.longitude_delta / 2.0,
        }
    }
}

/// The width and height of a map region in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateSpan {
    pub latitude_delta: f64,
    pub longitude_delta: f64,
}

impl CoordinateSpan {
    pub fn new(latitude_delta: f64, longitude_delta: f64) -> Self {
        Self {
            latitude_delta,
            longitude_delta,
        }
    }

    /// A span that covers roughly the given distance in meters on both axes
    /// at the given latitude.
    pub fn from_meters(meters: f64, latitude: f64) -> Self {
        let lat_delta = meters / 111_320.0;
        let lon_delta = meters / (111_320.0 * latitude.to_radians().cos().max(0.01));
        Self::new(lat_delta, lon_delta)
    }
}

/// A geographic bounding box in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub north: f64,
    pub south: f64,
    pub east: f64,
    pub west: f64,
}

impl BoundingBox {
    pub fn new(north: f64, south: f64, east: f64, west: f64) -> Self {
        Self {
            north,
            south,
            east,
            west,
        }
    }

    /// Smallest box containing both boxes.
    pub fn union(&self, other: &BoundingBox) -> BoundingBox {
        BoundingBox {
            north: self.north.max(other.north),
            south: self.south.min(other.south),
            east: self.east.max(other.east),
            west: self.west.min(other.west),
        }
    }

    /// Overpass QL bbox fragment: `(south,west,north,east)`.
    pub fn overpass_bbox(&self) -> String {
        format!(
            "({:.6},{:.6},{:.6},{:.6})",
            self.south, self.west, self.north, self.east
        )
    }
}

/// Categories a place can belong to. Each category maps to OpenStreetMap
/// amenity/shop/leisure tags used by the Overpass provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceCategory {
    Restaurant,
    Cafe,
    Bar,
    Hotel,
    Supermarket,
    Bakery,
    Pharmacy,
    Hospital,
    Doctor,
    Bank,
    Atm,
    Fuel,
    ChargingStation,
    Parking,
    BusStop,
    TrainStation,
    Airport,
    School,
    University,
    Museum,
    Cinema,
    Park,
    Playground,
    Library,
    PostOffice,
    Police,
    Toilet,
    Generic,
}

impl PlaceCategory {
    /// All categories, in display order.
    pub fn all() -> &'static [PlaceCategory] {
        &[
            PlaceCategory::Restaurant,
            PlaceCategory::Cafe,
            PlaceCategory::Bar,
            PlaceCategory::Hotel,
            PlaceCategory::Supermarket,
            PlaceCategory::Bakery,
            PlaceCategory::Pharmacy,
            PlaceCategory::Hospital,
            PlaceCategory::Doctor,
            PlaceCategory::Bank,
            PlaceCategory::Atm,
            PlaceCategory::Fuel,
            PlaceCategory::ChargingStation,
            PlaceCategory::Parking,
            PlaceCategory::BusStop,
            PlaceCategory::TrainStation,
            PlaceCategory::Airport,
            PlaceCategory::School,
            PlaceCategory::University,
            PlaceCategory::Museum,
            PlaceCategory::Cinema,
            PlaceCategory::Park,
            PlaceCategory::Playground,
            PlaceCategory::Library,
            PlaceCategory::PostOffice,
            PlaceCategory::Police,
            PlaceCategory::Toilet,
            PlaceCategory::Generic,
        ]
    }

    /// Overpass tag filter fragments for this category.
    pub fn overpass_filters(&self) -> &'static [&'static str] {
        match self {
            PlaceCategory::Restaurant => &["amenity=restaurant", "amenity=fast_food"],
            PlaceCategory::Cafe => &["amenity=cafe"],
            PlaceCategory::Bar => &["amenity=bar", "amenity=pub"],
            PlaceCategory::Hotel => &["tourism=hotel", "tourism=guest_house"],
            PlaceCategory::Supermarket => &["shop=supermarket"],
            PlaceCategory::Bakery => &["shop=bakery"],
            PlaceCategory::Pharmacy => &["amenity=pharmacy"],
            PlaceCategory::Hospital => &["amenity=hospital"],
            PlaceCategory::Doctor => &["amenity=doctors"],
            PlaceCategory::Bank => &["amenity=bank"],
            PlaceCategory::Atm => &["amenity=atm"],
            PlaceCategory::Fuel => &["amenity=fuel"],
            PlaceCategory::ChargingStation => &["amenity=charging_station"],
            PlaceCategory::Parking => &["amenity=parking"],
            PlaceCategory::BusStop => &["highway=bus_stop", "amenity=bus_station"],
            PlaceCategory::TrainStation => &["railway=station"],
            PlaceCategory::Airport => &["aeroway=aerodrome"],
            PlaceCategory::School => &["amenity=school"],
            PlaceCategory::University => &["amenity=university", "amenity=college"],
            PlaceCategory::Museum => &["tourism=museum"],
            PlaceCategory::Cinema => &["amenity=cinema"],
            PlaceCategory::Park => &["leisure=park"],
            PlaceCategory::Playground => &["leisure=playground"],
            PlaceCategory::Library => &["amenity=library"],
            PlaceCategory::PostOffice => &["amenity=post_office"],
            PlaceCategory::Police => &["amenity=police"],
            PlaceCategory::Toilet => &["amenity=toilets"],
            PlaceCategory::Generic => &[],
        }
    }

    /// Keyword used by the Photon fallback when searching for this category.
    pub fn photon_keyword(&self) -> Option<&'static str> {
        match self {
            PlaceCategory::Restaurant => Some("restaurant"),
            PlaceCategory::Cafe => Some("cafe"),
            PlaceCategory::Bar => Some("bar"),
            PlaceCategory::Hotel => Some("hotel"),
            PlaceCategory::Supermarket => Some("supermarket"),
            PlaceCategory::Bakery => Some("bakery"),
            PlaceCategory::Pharmacy => Some("pharmacy"),
            PlaceCategory::Hospital => Some("hospital"),
            PlaceCategory::Doctor => Some("doctor"),
            PlaceCategory::Bank => Some("bank"),
            PlaceCategory::Atm => Some("atm"),
            PlaceCategory::Fuel => Some("fuel"),
            PlaceCategory::ChargingStation => Some("charging station"),
            PlaceCategory::Parking => Some("parking"),
            PlaceCategory::BusStop => Some("bus stop"),
            PlaceCategory::TrainStation => Some("train station"),
            PlaceCategory::Airport => Some("airport"),
            PlaceCategory::School => Some("school"),
            PlaceCategory::University => Some("university"),
            PlaceCategory::Museum => Some("museum"),
            PlaceCategory::Cinema => Some("cinema"),
            PlaceCategory::Park => Some("park"),
            PlaceCategory::Playground => Some("playground"),
            PlaceCategory::Library => Some("library"),
            PlaceCategory::PostOffice => Some("post office"),
            PlaceCategory::Police => Some("police"),
            PlaceCategory::Toilet => Some("toilet"),
            PlaceCategory::Generic => None,
        }
    }

    /// Parses a category from an OSM tag key/value pair.
    pub fn from_osm_tag(key: &str, value: &str) -> Option<PlaceCategory> {
        match (key, value) {
            ("amenity", "restaurant") | ("amenity", "fast_food") => Some(PlaceCategory::Restaurant),
            ("amenity", "cafe") => Some(PlaceCategory::Cafe),
            ("amenity", "bar") | ("amenity", "pub") => Some(PlaceCategory::Bar),
            ("tourism", "hotel") | ("tourism", "guest_house") => Some(PlaceCategory::Hotel),
            ("shop", "supermarket") => Some(PlaceCategory::Supermarket),
            ("shop", "bakery") => Some(PlaceCategory::Bakery),
            ("amenity", "pharmacy") => Some(PlaceCategory::Pharmacy),
            ("amenity", "hospital") => Some(PlaceCategory::Hospital),
            ("amenity", "doctors") => Some(PlaceCategory::Doctor),
            ("amenity", "bank") => Some(PlaceCategory::Bank),
            ("amenity", "atm") => Some(PlaceCategory::Atm),
            ("amenity", "fuel") => Some(PlaceCategory::Fuel),
            ("amenity", "charging_station") => Some(PlaceCategory::ChargingStation),
            ("amenity", "parking") => Some(PlaceCategory::Parking),
            ("highway", "bus_stop") | ("amenity", "bus_station") => Some(PlaceCategory::BusStop),
            ("railway", "station") => Some(PlaceCategory::TrainStation),
            ("aeroway", "aerodrome") => Some(PlaceCategory::Airport),
            ("amenity", "school") => Some(PlaceCategory::School),
            ("amenity", "university") | ("amenity", "college") => Some(PlaceCategory::University),
            ("tourism", "museum") => Some(PlaceCategory::Museum),
            ("amenity", "cinema") => Some(PlaceCategory::Cinema),
            ("leisure", "park") => Some(PlaceCategory::Park),
            ("leisure", "playground") => Some(PlaceCategory::Playground),
            ("amenity", "library") => Some(PlaceCategory::Library),
            ("amenity", "post_office") => Some(PlaceCategory::PostOffice),
            ("amenity", "police") => Some(PlaceCategory::Police),
            ("amenity", "toilets") => Some(PlaceCategory::Toilet),
            _ => None,
        }
    }
}

/// A structured postal address.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Address {
    pub street: Option<String>,
    pub house_number: Option<String>,
    pub postcode: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    /// Single-line formatted address as returned by the provider.
    pub formatted: Option<String>,
}

impl Address {
    /// One-line representation, joining the available parts with commas.
    pub fn one_line(&self) -> String {
        if let Some(ref formatted) = self.formatted {
            return formatted.clone();
        }
        let mut parts: Vec<String> = Vec::new();
        if let (Some(street), Some(nr)) = (&self.street, &self.house_number) {
            parts.push(format!("{street} {nr}"));
        } else if let Some(street) = &self.street {
            parts.push(street.clone());
        }
        if let Some(pc) = &self.postcode {
            parts.push(pc.clone());
        }
        if let Some(city) = &self.city {
            parts.push(city.clone());
        }
        if let Some(country) = &self.country {
            parts.push(country.clone());
        }
        parts.join(", ")
    }
}

/// Extra details for a place, filled by `place_details`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaceInfo {
    pub phone: Option<String>,
    pub website: Option<String>,
    pub opening_hours: Option<String>,
    pub cuisine: Option<String>,
    pub wheelchair: Option<String>,
    pub internet_access: Option<String>,
    pub brand: Option<String>,
    pub operator: Option<String>,
}

/// A point of interest or search result (the MapKit `MKMapItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// Stable provider id (`nominatim:12345`, `osm:node/123`, ...).
    pub id: String,
    pub name: String,
    pub category: PlaceCategory,
    pub coordinate: Coordinate,
    pub address: Address,
    pub info: PlaceInfo,
    /// Distance in meters from the query reference point, when known.
    pub distance_m: Option<f64>,
}

impl Place {
    pub fn new(name: impl Into<String>, coordinate: Coordinate) -> Self {
        Self {
            id: String::new(),
            name: name.into(),
            category: PlaceCategory::Generic,
            coordinate,
            address: Address::default(),
            info: PlaceInfo::default(),
            distance_m: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_category(mut self, category: PlaceCategory) -> Self {
        self.category = category;
        self
    }

    pub fn with_address(mut self, address: Address) -> Self {
        self.address = address;
        self
    }

    pub fn with_info(mut self, info: PlaceInfo) -> Self {
        self.info = info;
        self
    }

    pub fn with_distance(mut self, meters: f64) -> Self {
        self.distance_m = Some(meters);
        self
    }
}

/// Travel mode for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TravelMode {
    Driving,
    Walking,
    Cycling,
}

impl Default for TravelMode {
    fn default() -> Self {
        Self::Driving
    }
}

impl TravelMode {
    /// OSRM profile name used by the primary provider.
    pub fn osrm_profile(&self) -> &'static str {
        match self {
            TravelMode::Driving => "driving",
            TravelMode::Walking => "foot",
            TravelMode::Cycling => "bike",
        }
    }

    /// FOSSGIS profile prefix used by the fallback provider.
    pub fn fossgis_profile(&self) -> &'static str {
        match self {
            TravelMode::Driving => "routed-car",
            TravelMode::Walking => "routed-foot",
            TravelMode::Cycling => "routed-bike",
        }
    }
}

/// One instruction of a route (the MapKit `MKRouteStep`).
#[derive(Debug, Clone, PartialEq)]
pub struct RouteStep {
    /// Human readable instruction, e.g. `"Turn left onto Hauptstraße"`.
    pub instruction: String,
    pub distance_m: f64,
    pub duration_s: f64,
    pub coordinate: Coordinate,
}

/// A computed route (the MapKit `MKRoute`).
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub distance_m: f64,
    pub duration_s: f64,
    pub geometry: Vec<Coordinate>,
    pub steps: Vec<RouteStep>,
    /// Name of the provider that computed the route.
    pub source: String,
}

impl Route {
    /// Distance formatted for display (`"12.4 km"` / `"850 m"`).
    pub fn distance_text(&self) -> String {
        format_distance(self.distance_m)
    }

    /// Duration formatted for display (`"1 h 23 min"` / `"12 min"`).
    pub fn duration_text(&self) -> String {
        format_duration(self.duration_s)
    }
}

/// Formats a distance in meters for display.
pub fn format_distance(meters: f64) -> String {
    if meters >= 1000.0 {
        format!("{:.1} km", meters / 1000.0)
    } else {
        format!("{} m", meters.round() as i64)
    }
}

/// Formats a duration in seconds for display.
pub fn format_duration(seconds: f64) -> String {
    let total_minutes = (seconds / 60.0).round() as i64;
    if total_minutes >= 60 {
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;
        if minutes == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {minutes} min")
        }
    } else {
        format!("{total_minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_distance() {
        // Berlin -> Hamburg is roughly 255 km.
        let berlin = Coordinate::new(52.52, 13.405);
        let hamburg = Coordinate::new(53.5511, 9.9937);
        let d = berlin.distance_to(&hamburg);
        assert!(d > 250_000.0 && d < 260_000.0, "distance was {d}");
    }

    #[test]
    fn region_bounding_box() {
        let region = MapRegion::new(
            Coordinate::new(10.0, 20.0),
            CoordinateSpan::new(2.0, 4.0),
        );
        let bbox = region.bounding_box();
        assert_eq!(bbox.north, 11.0);
        assert_eq!(bbox.south, 9.0);
        assert_eq!(bbox.east, 22.0);
        assert_eq!(bbox.west, 18.0);
        assert!(region.center.inside(&bbox));
    }

    #[test]
    fn span_from_meters() {
        let span = CoordinateSpan::from_meters(1113.2, 0.0);
        assert!((span.latitude_delta - 0.01).abs() < 1e-9);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_distance(850.0), "850 m");
        assert_eq!(format_distance(12_400.0), "12.4 km");
        assert_eq!(format_duration(720.0), "12 min");
        assert_eq!(format_duration(4980.0), "1 h 23 min");
    }

    #[test]
    fn place_builder() {
        let p = Place::new("Cafe Central", Coordinate::new(48.2, 16.37))
            .with_id("osm:node/1")
            .with_category(PlaceCategory::Cafe);
        assert_eq!(p.id, "osm:node/1");
        assert_eq!(p.category, PlaceCategory::Cafe);
    }

    #[test]
    fn travel_mode_profiles() {
        assert_eq!(TravelMode::Driving.osrm_profile(), "driving");
        assert_eq!(TravelMode::Walking.fossgis_profile(), "routed-foot");
    }
}
