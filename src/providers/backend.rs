//! Primary provider: the TontooOS backend maps API.
//!
//! All TontooOS map clients use the backend (`.../tontooos/api/maps`), which
//! proxies and caches upstream services so devices need no API keys and no
//! direct internet tile access:
//!
//! - Tiles: `/tiles/{street|satellite|dark}/...` (cached proxy)
//! - Search: `/search` (Nominatim with relevance scoring)
//! - Reverse geocoding: `/reverse` (Nominatim)
//! - Nearby places: `/places/nearby` (Overpass, distance-sorted)
//! - Place details: `/places/details` (Overpass + Wikidata)
//! - Routing: `/route` (OSRM, driving only)
//!
//! The provider sits first in the chain; when the backend is unreachable
//! every call fails fast and the chain falls through to the direct
//! providers. Base URL resolution: explicit
//! [`MapsConfiguration::backend_url`], then `TONTOO_BACKEND_URL`, then
//! localhost (see [`MapsConfiguration::resolved_backend_url`]).

use super::{get_with_query, http_client, response_json, MapProvider};
use crate::config::{MapStyle, MapsConfiguration};
use crate::error::MapsError;
use crate::types::{
    Address, Coordinate, Place, PlaceCategory, PlaceInfo, Route, RouteStep, TravelMode,
};
use serde_json::Value;

/// TontooOS backend backed provider (primary).
pub struct BackendProvider {
    base_url: String,
    user_agent: String,
    timeout_seconds: u64,
    style: MapStyle,
}

impl BackendProvider {
    pub fn new() -> Self {
        Self::with_url(
            MapsConfiguration::new().resolved_backend_url(),
            MapsConfiguration::new(),
        )
    }

    pub fn with_config(config: &MapsConfiguration) -> Self {
        Self::with_url(config.resolved_backend_url(), config.clone())
    }

    fn with_url(base_url: String, config: MapsConfiguration) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            user_agent: config.user_agent,
            timeout_seconds: config.timeout_seconds,
            style: config.style,
        }
    }

    fn client(&self) -> networkkit::http::HttpClient {
        http_client(&self.user_agent, self.timeout_seconds)
    }

    /// Tile source slug served for the configured style.
    fn tile_source(&self) -> &'static str {
        match self.style {
            MapStyle::Satellite => "satellite",
            MapStyle::Dark => "dark",
            _ => "street",
        }
    }

    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, MapsError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = get_with_query(&self.client(), &url, params)?;
        if !resp.is_success() {
            return Err(MapsError::Provider(format!(
                "backend returned status {} for {path}",
                resp.status
            )));
        }
        response_json(resp)
    }

    /// Backend `/search` / `/geocode` result to a place.
    fn result_to_place(item: &Value, reference: Option<Coordinate>) -> Option<Place> {
        let lat = item["lat"].as_f64()?;
        let lon = item["lon"].as_f64()?;
        let coordinate = Coordinate::new(lat, lon);
        let name = item["short_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| item["name"].as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return None;
        }
        let class = item["category"].as_str().unwrap_or("");
        let typ = item["type"].as_str().unwrap_or("");
        let category =
            PlaceCategory::from_osm_tag(class, typ).unwrap_or(PlaceCategory::Generic);
        let address = nominatim_address(&item["address"]);
        let extra = &item["extratags"];
        let info = PlaceInfo {
            phone: opt_str(extra, "phone"),
            website: opt_str(extra, "website"),
            opening_hours: opt_str(extra, "opening_hours"),
            cuisine: opt_str(extra, "cuisine"),
            wheelchair: opt_str(extra, "wheelchair"),
            internet_access: opt_str(extra, "internet_access"),
            brand: opt_str(extra, "brand"),
            operator: opt_str(extra, "operator"),
        };
        let id = item["place_id"]
            .as_i64()
            .map(|id| format!("tontoo:{id}"))
            .unwrap_or_default();
        let mut place = Place::new(name, coordinate)
            .with_id(id)
            .with_category(category)
            .with_address(address)
            .with_info(info);
        if let Some(center) = reference {
            place = place.with_distance(coordinate.distance_to(&center));
        }
        Some(place)
    }
}

impl Default for BackendProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Backend POI category name for a place category, if the backend serves it.
fn backend_category(category: PlaceCategory) -> Option<&'static str> {
    match category {
        PlaceCategory::Restaurant => Some("restaurant"),
        PlaceCategory::Cafe => Some("cafe"),
        PlaceCategory::Bar => Some("bar"),
        PlaceCategory::Hotel => Some("hotel"),
        PlaceCategory::Supermarket => Some("supermarket"),
        PlaceCategory::Pharmacy => Some("pharmacy"),
        PlaceCategory::Hospital => Some("hospital"),
        PlaceCategory::Bank => Some("bank"),
        PlaceCategory::Fuel => Some("fuel"),
        PlaceCategory::Parking => Some("parking"),
        PlaceCategory::BusStop => Some("bus_stop"),
        PlaceCategory::TrainStation => Some("train_station"),
        PlaceCategory::Airport => Some("airport"),
        PlaceCategory::School => Some("school"),
        PlaceCategory::University => Some("university"),
        PlaceCategory::Museum => Some("museum"),
        PlaceCategory::Cinema => Some("cinema"),
        PlaceCategory::Park => Some("park"),
        PlaceCategory::Playground => Some("playground"),
        PlaceCategory::Library => Some("library"),
        _ => None,
    }
}

/// Best-effort category from a backend nearby result, which only carries
/// the OSM value (e.g. `"restaurant"`) without the key.
fn category_from_backend_value(value: &str) -> Option<PlaceCategory> {
    ["amenity", "shop", "tourism", "leisure", "railway", "aeroway", "highway"]
        .iter()
        .find_map(|key| PlaceCategory::from_osm_tag(key, value))
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Nominatim addressdetails object to an [`Address`].
fn nominatim_address(address: &Value) -> Address {
    let opt = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| address[k].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Address {
        street: opt(&["road", "street", "footway", "path"]),
        house_number: opt(&["house_number"]),
        postcode: opt(&["postcode"]),
        city: opt(&["city", "town", "village", "municipality", "county"]),
        state: opt(&["state", "region"]),
        country: opt(&["country"]),
        country_code: opt(&["country_code"]),
        formatted: None,
    }
}

/// Parses a `osm:{N|W|R}{id}` place id into backend query parameters
/// (`osm_type`, `osm_id`).
fn parse_osm_id(id: &str) -> Option<(&'static str, String)> {
    let rest = id.strip_prefix("osm:")?;
    let (letter, digits) = rest.split_at(1);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let osm_type = match letter {
        "N" => "node",
        "W" => "way",
        "R" => "relation",
        _ => return None,
    };
    Some((osm_type, digits.to_string()))
}

impl MapProvider for BackendProvider {
    fn name(&self) -> &'static str {
        "TontooOS Backend"
    }

    /// Serves every style through the backend tile proxy.
    fn supports_style(&self, style: MapStyle) -> bool {
        let _ = style;
        true
    }

    fn is_available(&self) -> bool {
        let client = self.client();
        let url = format!("{}/tiles/sources", self.base_url);
        client
            .get(&url)
            .send()
            .map(|r| r.is_success())
            .unwrap_or(false)
    }

    fn search(
        &self,
        query: &str,
        near: Option<Coordinate>,
        limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        let json = self.get(
            "/search",
            &[
                ("q", query.to_string()),
                ("limit", limit.clamp(1, 50).to_string()),
            ],
        )?;
        let mut places: Vec<Place> = json["results"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Self::result_to_place(item, near))
                    .collect()
            })
            .unwrap_or_default();
        // Nearest first when searching from a reference point.
        if near.is_some() {
            places.sort_by(|a, b| {
                a.distance_m
                    .partial_cmp(&b.distance_m)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        places.truncate(limit.clamp(1, 50));
        Ok(places)
    }

    fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError> {
        let json = self.get(
            "/reverse",
            &[
                ("lat", coordinate.latitude.to_string()),
                ("lon", coordinate.longitude.to_string()),
            ],
        )?;
        let mut address = nominatim_address(&json["address"]);
        address.formatted = json["name"].as_str().map(str::to_string);
        Ok(address)
    }

    fn place_details(&self, place: &Place) -> Result<Place, MapsError> {
        let Some((osm_type, osm_id)) = parse_osm_id(&place.id) else {
            return Ok(place.clone());
        };
        let json = self.get(
            "/places/details",
            &[("osm_type", osm_type.to_string()), ("osm_id", osm_id)],
        )?;
        let mut enriched = place.clone();
        enriched.info = PlaceInfo {
            phone: opt_str(&json, "phone"),
            website: opt_str(&json, "website"),
            opening_hours: opt_str(&json, "opening_hours"),
            cuisine: opt_str(&json, "cuisine"),
            wheelchair: opt_str(&json, "wheelchair"),
            internet_access: opt_str(&json, "wifi"),
            brand: opt_str(&json, "brand"),
            operator: opt_str(&json, "operator"),
        };
        if enriched.address.formatted.is_none() {
            enriched.address.formatted = opt_str(&json, "address");
        }
        Ok(enriched)
    }

    fn nearby(
        &self,
        center: Coordinate,
        radius_m: u32,
        categories: &[PlaceCategory],
        limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        if categories.is_empty() {
            return Err(MapsError::InvalidQuery(
                "at least one category is required".into(),
            ));
        }
        let mut all = Vec::new();
        for category in categories {
            let Some(backend_name) = backend_category(*category) else {
                continue;
            };
            let json = self.get(
                "/places/nearby",
                &[
                    ("lat", center.latitude.to_string()),
                    ("lon", center.longitude.to_string()),
                    ("radius", radius_m.min(10000).to_string()),
                    ("category", backend_name.to_string()),
                    ("limit", limit.clamp(1, 50).to_string()),
                ],
            )?;
            let items = json["results"].as_array().cloned().unwrap_or_default();
            for item in items {
                let (Some(lat), Some(lon)) = (item["lat"].as_f64(), item["lon"].as_f64()) else {
                    continue;
                };
                let coordinate = Coordinate::new(lat, lon);
                let name = item["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                let resolved = item["category"]
                    .as_str()
                    .and_then(category_from_backend_value)
                    .unwrap_or(*category);
                let address = Address {
                    formatted: opt_str(&item, "address"),
                    ..Address::default()
                };
                let info = PlaceInfo {
                    phone: opt_str(&item, "phone"),
                    website: opt_str(&item, "website"),
                    opening_hours: opt_str(&item, "opening_hours"),
                    cuisine: opt_str(&item, "cuisine"),
                    wheelchair: opt_str(&item, "wheelchair"),
                    internet_access: None,
                    brand: None,
                    operator: None,
                };
                let id = match (item["osm_type"].as_str(), item["osm_id"].as_i64()) {
                    (Some("way"), Some(id)) => format!("osm:W{id}"),
                    (Some("relation"), Some(id)) => format!("osm:R{id}"),
                    (_, Some(id)) => format!("osm:N{id}"),
                    _ => String::new(),
                };
                let mut place = Place::new(name, coordinate)
                    .with_id(id)
                    .with_category(resolved)
                    .with_address(address)
                    .with_info(info);
                match item["distance_m"].as_f64() {
                    Some(d) => place = place.with_distance(d),
                    None => {
                        place = place.with_distance(coordinate.distance_to(&center));
                    }
                }
                all.push(place);
            }
        }
        all.sort_by(|a, b| {
            a.distance_m
                .partial_cmp(&b.distance_m)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(limit.clamp(1, 50));
        Ok(all)
    }

    fn route(
        &self,
        from: Coordinate,
        to: Coordinate,
        mode: TravelMode,
    ) -> Result<Route, MapsError> {
        // The backend routes driving only; other modes fall through to the
        // direct providers via the chain.
        if mode != TravelMode::Driving {
            return Err(MapsError::Provider("backend routes driving only".into()));
        }
        let json = self.get(
            "/route",
            &[
                (
                    "from",
                    format!("{},{}", from.latitude, from.longitude),
                ),
                ("to", format!("{},{}", to.latitude, to.longitude)),
            ],
        )?;
        let geometry: Vec<Coordinate> = json["geometry"]
            .as_array()
            .map(|points| {
                points
                    .iter()
                    .filter_map(|p| {
                        Some(Coordinate::new(p["lat"].as_f64()?, p["lon"].as_f64()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let steps: Vec<RouteStep> = json["steps"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|s| {
                        let road = s["name"].as_str().unwrap_or("");
                        let maneuver = serde_json::json!({
                            "type": s["instruction"].as_str().unwrap_or(""),
                            "modifier": s["modifier"].as_str().unwrap_or(""),
                        });
                        let (Some(lat), Some(lon)) = (
                            s["start"]["lat"].as_f64(),
                            s["start"]["lon"].as_f64(),
                        ) else {
                            return RouteStep {
                                instruction: super::osm::instruction_from_osrm(
                                    &maneuver, road,
                                ),
                                distance_m: s["distance"].as_f64().unwrap_or(0.0),
                                duration_s: s["duration"].as_f64().unwrap_or(0.0),
                                coordinate: from,
                            };
                        };
                        RouteStep {
                            instruction: super::osm::instruction_from_osrm(&maneuver, road),
                            distance_m: s["distance"].as_f64().unwrap_or(0.0),
                            duration_s: s["duration"].as_f64().unwrap_or(0.0),
                            coordinate: Coordinate::new(lat, lon),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Route {
            distance_m: json["distance"].as_f64().unwrap_or(0.0),
            duration_s: json["duration"].as_f64().unwrap_or(0.0),
            geometry,
            steps,
            source: self.name().to_string(),
        })
    }

    /// Proxied tiles: `/tiles/{street|satellite|dark}/...`.
    fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
        format!(
            "{}/tiles/{}/{z}/{x}/{y}",
            self.base_url,
            self.tile_source()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_BACKEND_URL;

    #[test]
    fn tile_urls_follow_style() {
        let street =
            BackendProvider::with_config(&MapsConfiguration::new().style(MapStyle::Light));
        assert!(street
            .tile_url(5, 6, 7)
            .ends_with("/tiles/street/7/5/6"));

        let satellite =
            BackendProvider::with_config(&MapsConfiguration::new().style(MapStyle::Satellite));
        assert!(satellite
            .tile_url(5, 6, 7)
            .ends_with("/tiles/satellite/7/5/6"));

        let dark =
            BackendProvider::with_config(&MapsConfiguration::new().style(MapStyle::Dark));
        assert!(dark
            .tile_url(5, 6, 7)
            .ends_with("/tiles/dark/7/5/6"));
    }

    #[test]
    fn base_url_normalization() {
        let config = MapsConfiguration::new().backend_url("http://example.com/maps/");
        let p = BackendProvider::with_config(&config);
        assert!(p.tile_url(1, 2, 3).starts_with("http://example.com/maps/tiles/"));
    }

    #[test]
    fn env_override_for_base_url() {
        std::env::set_var("TONTOO_BACKEND_URL", "http://backend.local/maps");
        let url = MapsConfiguration::new().resolved_backend_url();
        assert_eq!(url, "http://backend.local/maps");
        std::env::remove_var("TONTOO_BACKEND_URL");
        assert_eq!(
            MapsConfiguration::new().resolved_backend_url(),
            DEFAULT_BACKEND_URL
        );
    }

    #[test]
    fn backend_categories_cover_common_cases() {
        assert_eq!(backend_category(PlaceCategory::Restaurant), Some("restaurant"));
        assert_eq!(backend_category(PlaceCategory::TrainStation), Some("train_station"));
        assert_eq!(backend_category(PlaceCategory::Generic), None);
        assert_eq!(backend_category(PlaceCategory::Atm), None);
    }

    #[test]
    fn osm_id_roundtrip() {
        assert_eq!(
            parse_osm_id("osm:N123"),
            Some(("node", "123".to_string()))
        );
        assert_eq!(
            parse_osm_id("osm:W456"),
            Some(("way", "456".to_string()))
        );
        assert_eq!(parse_osm_id("tontoo:99"), None);
        assert_eq!(parse_osm_id("osm:X1"), None);
        assert_eq!(parse_osm_id("osm:N"), None);
    }

    #[test]
    fn search_result_mapping() {
        let item = serde_json::json!({
            "short_name": "Aldi",
            "lat": 52.52,
            "lon": 13.405,
            "type": "supermarket",
            "category": "shop",
            "place_id": 42,
            "address": { "road": "Hauptstraße", "house_number": "1", "postcode": "10115", "city": "Berlin", "country": "Germany" },
            "extratags": { "opening_hours": "Mo-Sa 08:00-20:00" }
        });
        let place = BackendProvider::result_to_place(&item, Some(Coordinate::new(52.0, 13.0)))
            .expect("maps");
        assert_eq!(place.name, "Aldi");
        assert_eq!(place.category, PlaceCategory::Supermarket);
        assert_eq!(place.id, "tontoo:42");
        assert_eq!(place.address.city.as_deref(), Some("Berlin"));
        assert_eq!(
            place.info.opening_hours.as_deref(),
            Some("Mo-Sa 08:00-20:00")
        );
        assert!(place.distance_m.is_some());
    }

    #[test]
    fn style_support() {
        let p = BackendProvider::new();
        assert!(p.supports_style(MapStyle::Light));
        assert!(p.supports_style(MapStyle::Dark));
        assert!(p.supports_style(MapStyle::Standard));
        assert!(p.supports_style(MapStyle::Satellite));
    }
}
