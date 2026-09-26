//! Primary provider: OpenStreetMap.
//!
//! - Search / geocoding: Nominatim (`nominatim.openstreetmap.org`)
//! - Place details: Nominatim lookup with `extratags`
//! - Nearby places: Overpass API (`overpass-api.de`)
//! - Routing: OSRM demo server (`router.project-osrm.org`)
//! - Tiles: `tile.openstreetmap.org` (standard cartography)

use super::{get_with_query, http_client, response_json, MapProvider};
use crate::config::MapStyle;
use crate::error::MapsError;
use crate::types::{
    Address, Coordinate, Place, PlaceCategory, PlaceInfo, Route, RouteStep, TravelMode,
};
use serde_json::Value;

/// OpenStreetMap-backed provider (primary).
pub struct OsmProvider {
    user_agent: String,
    timeout_seconds: u64,
    overpass_url: String,
    nominatim_url: String,
    osrm_url: String,
}

impl OsmProvider {
    pub fn new() -> Self {
        Self {
            user_agent: format!("TontooOS-MapsKit/{}", crate::MAPSKIT_VERSION_STR),
            timeout_seconds: 10,
            overpass_url: "https://overpass-api.de/api/interpreter".into(),
            nominatim_url: "https://nominatim.openstreetmap.org".into(),
            osrm_url: "https://router.project-osrm.org".into(),
        }
    }

    pub fn with_config(config: &crate::config::MapsConfiguration) -> Self {
        Self {
            user_agent: config.user_agent.clone(),
            timeout_seconds: config.timeout_seconds,
            ..Self::new()
        }
    }

    fn client(&self) -> networkkit::http::HttpClient {
        http_client(&self.user_agent, self.timeout_seconds)
    }
}

impl Default for OsmProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MapProvider for OsmProvider {
    fn name(&self) -> &'static str {
        "OpenStreetMap"
    }

    /// Serves the classic standard cartography.
    fn supports_style(&self, style: MapStyle) -> bool {
        matches!(style, MapStyle::Standard)
    }

    fn is_available(&self) -> bool {
        networkkit::http::HttpRequest::new(
            networkkit::http::HttpMethod::Head,
            &format!("{}/status", self.nominatim_url),
        )
        .timeout(std::time::Duration::from_secs(self.timeout_seconds))
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
        let client = self.client();
        let mut params = vec![
            ("q", query.to_string()),
            ("format", "jsonv2".to_string()),
            ("addressdetails", "1".to_string()),
            ("limit", limit.clamp(1, 50).to_string()),
        ];

        // Bias results towards the reference area without hard-bounding.
        if let Some(center) = near {
            let d = 2.0;
            params.push((
                "viewbox",
                format!(
                    "{:.4},{:.4},{:.4},{:.4}",
                    center.longitude - d,
                    center.latitude + d,
                    center.longitude + d,
                    center.latitude - d
                ),
            ));
        }

        let resp = get_with_query(&client, &format!("{}/search", self.nominatim_url), &params)?;
        if !resp.is_success() {
            return Err(MapsError::Provider(format!(
                "Nominatim returned status {}",
                resp.status
            )));
        }
        let json: Value = response_json(resp)?;

        let mut places = Vec::new();
        if let Some(items) = json.as_array() {
            for item in items {
                let lat = item["lat"].as_str().and_then(|s| s.parse::<f64>().ok());
                let lon = item["lon"].as_str().and_then(|s| s.parse::<f64>().ok());
                let (Some(lat), Some(lon)) = (lat, lon) else {
                    continue;
                };
                let coordinate = Coordinate::new(lat, lon);

                let name = item["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        item["display_name"]
                            .as_str()
                            .unwrap_or("Unknown")
                            .split(',')
                            .next()
                            .unwrap_or("Unknown")
                            .trim()
                            .to_string()
                    });

                let category = classify_nominatim(item);
                let address = parse_nominatim_address(item);
                let osm_type = item["osm_type"].as_str().unwrap_or("N").to_uppercase();
                let osm_id = item["osm_id"].as_i64().unwrap_or(0);
                let id = format!("nominatim:{osm_type}{osm_id}");

                places.push(
                    Place::new(name, coordinate)
                        .with_id(id)
                        .with_category(category)
                        .with_address(address),
                );
            }
        }
        Ok(places)
    }

    fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError> {
        let client = self.client();
        let resp = get_with_query(
            &client,
            &format!("{}/reverse", self.nominatim_url),
            &[
                ("lat", coordinate.latitude.to_string()),
                ("lon", coordinate.longitude.to_string()),
                ("format", "jsonv2".into()),
                ("addressdetails", "1".into()),
                ("zoom", "18".into()),
            ],
        )?;

        if !resp.is_success() {
            return Err(MapsError::Provider(format!(
                "Nominatim returned status {}",
                resp.status
            )));
        }
        let json: Value = response_json(resp)?;
        if json["error"].is_object() || json["error"].is_string() {
            return Err(MapsError::Provider(
                json["error"].as_str().unwrap_or("reverse geocoding failed").into(),
            ));
        }
        Ok(parse_nominatim_address(&json))
    }

    fn place_details(&self, place: &Place) -> Result<Place, MapsError> {
        // Details come from a Nominatim lookup with extratags.
        let Some(osm_ref) = place.id.strip_prefix("nominatim:") else {
            return Ok(place.clone());
        };
        if osm_ref.is_empty() {
            return Ok(place.clone());
        }

        let client = self.client();
        let resp = get_with_query(
            &client,
            &format!("{}/lookup", self.nominatim_url),
            &[
                ("osm_ids", osm_ref.to_string()),
                ("format", "jsonv2".into()),
                ("addressdetails", "1".into()),
                ("extratags", "1".into()),
            ],
        )?;

        if !resp.is_success() {
            return Err(MapsError::Provider(format!(
                "Nominatim returned status {}",
                resp.status
            )));
        }
        let json: Value = response_json(resp)?;
        let item = json.as_array().and_then(|a| a.first()).cloned();
        let Some(item) = item else {
            return Ok(place.clone());
        };

        let tags = &item["extratags"];
        let get = |key: &str| {
            tags[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };

        let info = PlaceInfo {
            phone: get("phone").or_else(|| get("contact:phone")),
            website: get("website").or_else(|| get("contact:website")),
            opening_hours: get("opening_hours"),
            cuisine: get("cuisine"),
            wheelchair: get("wheelchair"),
            internet_access: get("internet_access"),
            brand: get("brand"),
            operator: get("operator"),
        };

        let mut enriched = place.clone();
        enriched.info = info;
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

        // Build the Overpass QL query.
        let mut filters = String::new();
        for category in categories {
            for filter in category.overpass_filters() {
                let (key, value) = filter.split_once('=').unwrap_or((filter, ""));
                filters.push_str(&format!(
                    "  node[\"{key}\"=\"{value}\"](around:{radius_m},{:.6},{:.6});\n",
                    center.latitude, center.longitude
                ));
                filters.push_str(&format!(
                    "  way[\"{key}\"=\"{value}\"](around:{radius_m},{:.6},{:.6});\n",
                    center.latitude, center.longitude
                ));
            }
        }
        let body = format!("[out:json][timeout:{}];\n(\n{filters});\nout center {};",
            self.timeout_seconds.min(60),
            limit.clamp(1, 100)
        );

        let client = self.client();
        let resp = client
            .post(&self.overpass_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body_str(&format!("data={}", urlencode(&body)))
            .send()?;

        if !resp.is_success() {
            return Err(MapsError::Provider(format!(
                "Overpass returned status {}",
                resp.status
            )));
        }
        let json: Value = response_json(resp)?;

        let mut places = Vec::new();
        if let Some(elements) = json["elements"].as_array() {
            for element in elements {
                let lat = element["lat"]
                    .as_f64()
                    .or_else(|| element["center"]["lat"].as_f64());
                let lon = element["lon"]
                    .as_f64()
                    .or_else(|| element["center"]["lon"].as_f64());
                let (Some(lat), Some(lon)) = (lat, lon) else {
                    continue;
                };
                let coordinate = Coordinate::new(lat, lon);
                let tags = &element["tags"];

                let name = tags["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let Some(name) = name else {
                    continue;
                };

                let mut category = PlaceCategory::Generic;
                if let Some(tags) = tags.as_object() {
                    for (key, value) in tags {
                        let Some(value) = value.as_str() else {
                            continue;
                        };
                        if let Some(c) = PlaceCategory::from_osm_tag(key, value) {
                            category = c;
                            break;
                        }
                    }
                }

                let get = |key: &str| {
                    tags[key]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                let address = Address {
                    street: get("addr:street"),
                    house_number: get("addr:housenumber"),
                    postcode: get("addr:postcode"),
                    city: get("addr:city"),
                    state: None,
                    country: None,
                    country_code: None,
                    formatted: None,
                };
                let info = PlaceInfo {
                    phone: get("phone").or_else(|| get("contact:phone")),
                    website: get("website").or_else(|| get("contact:website")),
                    opening_hours: get("opening_hours"),
                    cuisine: get("cuisine"),
                    wheelchair: get("wheelchair"),
                    internet_access: get("internet_access"),
                    brand: get("brand"),
                    operator: get("operator"),
                };

                let osm_type = element["type"].as_str().unwrap_or("node");
                let osm_id = element["id"].as_i64().unwrap_or(0);
                let letter = match osm_type {
                    "way" => "W",
                    "relation" => "R",
                    _ => "N",
                };

                places.push(
                    Place::new(name, coordinate)
                        .with_id(format!("osm:{letter}{osm_id}"))
                        .with_category(category)
                        .with_address(address)
                        .with_info(info)
                        .with_distance(coordinate.distance_to(&center)),
                );
            }
        }

        places.sort_by(|a, b| {
            a.distance_m
                .partial_cmp(&b.distance_m)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        places.truncate(limit);
        Ok(places)
    }

    fn route(
        &self,
        from: Coordinate,
        to: Coordinate,
        mode: TravelMode,
    ) -> Result<Route, MapsError> {
        fetch_osrm_route(
            &self.client(),
            &self.osrm_url,
            mode.osrm_profile(),
            from,
            to,
            self.name(),
        )
    }

    fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
        format!("https://tile.openstreetmap.org/{z}/{x}/{y}.png")
    }
}

/// Shared OSRM route fetching used by both providers.
pub(crate) fn fetch_osrm_route(
    client: &networkkit::http::HttpClient,
    base_url: &str,
    profile: &str,
    from: Coordinate,
    to: Coordinate,
    source_name: &'static str,
) -> Result<Route, MapsError> {
    let coords = format!(
        "{:.6},{:.6};{:.6},{:.6}",
        from.longitude, from.latitude, to.longitude, to.latitude
    );
    let url = format!(
        "{base_url}/route/v1/{profile}/{coords}?overview=full&geometries=geojson&steps=true"
    );

    let resp = client.get(&url).send()?;
    if !resp.is_success() {
        return Err(MapsError::Provider(format!(
            "OSRM returned status {}",
            resp.status
        )));
    }
    let json: Value = response_json(resp)?;

    if json["code"].as_str() != Some("Ok") {
        return Err(MapsError::Provider(format!(
            "OSRM code: {}",
            json["code"].as_str().unwrap_or("unknown")
        )));
    }

    let route = json["routes"]
        .as_array()
        .and_then(|r| r.first())
        .ok_or_else(|| MapsError::Parse("OSRM response has no routes".into()))?;

    let distance_m = route["distance"].as_f64().unwrap_or(0.0);
    let duration_s = route["duration"].as_f64().unwrap_or(0.0);

    let mut geometry = Vec::new();
    if let Some(coords) = route["geometry"]["coordinates"].as_array() {
        for pair in coords {
            if let (Some(lon), Some(lat)) = (pair[0].as_f64(), pair[1].as_f64()) {
                geometry.push(Coordinate::new(lat, lon));
            }
        }
    }

    let mut steps = Vec::new();
    if let Some(legs) = route["legs"].as_array() {
        for leg in legs {
            if let Some(leg_steps) = leg["steps"].as_array() {
                for step in leg_steps {
                    let lon = step["maneuver"]["location"][0].as_f64().unwrap_or(0.0);
                    let lat = step["maneuver"]["location"][1].as_f64().unwrap_or(0.0);
                    let road = step["name"].as_str().unwrap_or("").to_string();
                    steps.push(RouteStep {
                        instruction: instruction_from_osrm(step, &road),
                        distance_m: step["distance"].as_f64().unwrap_or(0.0),
                        duration_s: step["duration"].as_f64().unwrap_or(0.0),
                        coordinate: Coordinate::new(lat, lon),
                    });
                }
            }
        }
    }

    Ok(Route {
        distance_m,
        duration_s,
        geometry,
        steps,
        source: source_name.to_string(),
    })
}

fn instruction_from_osrm(step: &Value, road: &str) -> String {
    let maneuver = &step["maneuver"];
    let mtype = maneuver["type"].as_str().unwrap_or("");
    let modifier = maneuver["modifier"].as_str().unwrap_or("");

    let turn_word = match modifier {
        "left" => "Turn left",
        "right" => "Turn right",
        "slight left" => "Bear left",
        "slight right" => "Bear right",
        "sharp left" => "Sharp left",
        "sharp right" => "Sharp right",
        "uturn" => "Make a U-turn",
        _ => "",
    };
    let onto = if road.is_empty() {
        String::new()
    } else {
        format!(" onto {road}")
    };

    match mtype {
        "depart" => format!("Start{}", onto),
        "arrive" => "Arrive at your destination".to_string(),
        "roundabout" | "rotary" | "roundabout turn" => format!("Take the roundabout{}", onto),
        "merge" => format!("Merge{}", onto),
        "on ramp" => format!("Take the ramp{}", onto),
        "off ramp" => format!("Take the exit{}", onto),
        "fork" => {
            if turn_word.is_empty() {
                format!("Keep straight at the fork{}", onto)
            } else {
                format!("{turn_word} at the fork{}", onto)
            }
        }
        "end of road" => {
            if turn_word.is_empty() {
                format!("Continue{}", onto)
            } else {
                format!("{turn_word} at the end of the road{}", onto)
            }
        }
        "new name" | "continue" => format!("Continue{}", onto),
        _ => {
            if turn_word.is_empty() {
                format!("Continue{}", onto)
            } else {
                format!("{turn_word}{}", onto)
            }
        }
    }
}

/// Classifies a Nominatim result by its `class`/`type` pair.
fn classify_nominatim(item: &Value) -> PlaceCategory {
    let class = item["class"].as_str().unwrap_or("");
    let typ = item["type"].as_str().unwrap_or("");
    PlaceCategory::from_osm_tag(class, typ).unwrap_or(match (class, typ) {
        ("highway", "bus_stop") => PlaceCategory::BusStop,
        ("place", _) => PlaceCategory::Generic,
        ("boundary", _) | ("admin_level", _) => PlaceCategory::Generic,
        _ => PlaceCategory::Generic,
    })
}

/// Parses the `address` object of a Nominatim jsonv2 response.
fn parse_nominatim_address(item: &Value) -> Address {
    let addr = &item["address"];
    let get = |key: &str| {
        addr[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Address {
        street: get("road").or_else(|| get("pedestrian")).or_else(|| get("footway")),
        house_number: get("house_number"),
        postcode: get("postcode"),
        city: get("city")
            .or_else(|| get("town"))
            .or_else(|| get("village"))
            .or_else(|| get("municipality")),
        state: get("state"),
        country: get("country"),
        country_code: get("country_code"),
        formatted: item["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

/// Minimal percent-encoding for Overpass QL bodies.
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_urls() {
        let p = OsmProvider::new();
        assert_eq!(
            p.tile_url(1, 2, 3),
            "https://tile.openstreetmap.org/3/1/2.png"
        );
    }

    #[test]
    fn style_support() {
        let p = OsmProvider::new();
        assert!(p.supports_style(MapStyle::Standard));
        assert!(!p.supports_style(MapStyle::Light));
        assert!(!p.supports_style(MapStyle::Dark));
    }

    #[test]
    fn instructions() {
        let step = serde_json::json!({
            "maneuver": { "type": "turn", "modifier": "left", "location": [13.4, 52.5] },
            "name": "Hauptstraße",
            "distance": 120.0,
            "duration": 30.0
        });
        assert_eq!(
            instruction_from_osrm(&step, "Hauptstraße"),
            "Turn left onto Hauptstraße"
        );

        let depart = serde_json::json!({
            "maneuver": { "type": "depart", "modifier": "", "location": [0.0, 0.0] },
            "name": ""
        });
        assert_eq!(instruction_from_osrm(&depart, ""), "Start");
    }

    #[test]
    fn urlencode_basic() {
        assert_eq!(urlencode("[out:json];"), "%5Bout%3Ajson%5D%3B");
    }

    #[test]
    fn classify() {
        let item = serde_json::json!({ "class": "amenity", "type": "restaurant" });
        assert_eq!(classify_nominatim(&item), PlaceCategory::Restaurant);
    }
}
