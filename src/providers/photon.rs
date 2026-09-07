//! Fallback provider: Komoot Photon + CARTO basemaps.
//!
//! - Search / geocoding: Photon (`photon.komoot.io`)
//! - Nearby places: Photon category keyword search around a center
//! - Routing: FOSSGIS OSRM instances (`routing.openstreetmap.de`)
//! - Tiles: CARTO basemaps (`basemaps.cartocdn.com`), Voyager (light) and
//!   Dark Matter (dark)

use super::osm::fetch_osrm_route;
use super::{http_client, MapProvider};
use crate::config::{MapStyle, MapsConfiguration};
use crate::error::MapsError;
use crate::types::{
    Address, Coordinate, Place, PlaceCategory, PlaceInfo, Route, TravelMode,
};
use serde_json::Value;

/// Photon + CARTO backed provider (fallback).
pub struct PhotonProvider {
    user_agent: String,
    timeout_seconds: u64,
    photon_url: String,
    fossgis_url: String,
    style: MapStyle,
}

impl PhotonProvider {
    pub fn new() -> Self {
        Self {
            user_agent: format!("TontooOS-MapsKit/{}", crate::MAPSKIT_VERSION_STR),
            timeout_seconds: 10,
            photon_url: "https://photon.komoot.io".into(),
            fossgis_url: "https://routing.openstreetmap.de".into(),
            style: MapStyle::Light,
        }
    }

    pub fn with_config(config: &MapsConfiguration) -> Self {
        Self {
            user_agent: config.user_agent.clone(),
            timeout_seconds: config.timeout_seconds,
            style: config.style,
            ..Self::new()
        }
    }

    fn client(&self) -> reqwest::blocking::Client {
        http_client(&self.user_agent, self.timeout_seconds)
    }

    /// CARTO basemap slug for the configured style.
    ///
    /// The light style uses CARTO Voyager, whose colorful cartography
    /// (green parks, blue water, orange roads) follows the Apple Maps look.
    fn carto_slug(&self) -> &'static str {
        match self.style {
            MapStyle::Dark => "dark_all",
            _ => "rastertiles/voyager",
        }
    }

    fn query(&self, params: &[(&str, String)]) -> Result<Value, MapsError> {
        let client = self.client();
        let resp = client
            .get(format!("{}/api/", self.photon_url))
            .query(params)
            .send()?;
        if !resp.status().is_success() {
            return Err(MapsError::Provider(format!(
                "Photon returned status {}",
                resp.status()
            )));
        }
        resp.json().map_err(MapsError::from)
    }

    fn features_to_places(
        &self,
        json: &Value,
        reference: Option<Coordinate>,
    ) -> Vec<Place> {
        let mut places = Vec::new();
        let Some(features) = json["features"].as_array() else {
            return places;
        };
        for feature in features {
            let lon = feature["geometry"]["coordinates"][0].as_f64();
            let lat = feature["geometry"]["coordinates"][1].as_f64();
            let (Some(lon), Some(lat)) = (lon, lat) else {
                continue;
            };
            let coordinate = Coordinate::new(lat, lon);
            let props = &feature["properties"];

            let name = props["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let Some(name) = name else { continue };

            let osm_key = props["osm_key"].as_str().unwrap_or("");
            let osm_value = props["osm_value"].as_str().unwrap_or("");
            let category =
                PlaceCategory::from_osm_tag(osm_key, osm_value).unwrap_or(PlaceCategory::Generic);

            let opt = |key: &str| {
                props[key]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let address = Address {
                street: opt("street"),
                house_number: opt("housenumber"),
                postcode: opt("postcode"),
                city: opt("city").or_else(|| opt("district")),
                state: opt("state"),
                country: opt("country"),
                country_code: opt("countrycode"),
                formatted: None,
            };

            let osm_type = props["osm_type"].as_str().unwrap_or("N");
            let osm_id = props["osm_id"].as_i64().unwrap_or(0);
            let letter = match osm_type {
                "W" => "W",
                "R" => "R",
                _ => "N",
            };
            let id = if osm_id != 0 {
                format!("osm:{letter}{osm_id}")
            } else {
                format!("photon:{lat:.6},{lon:.6}")
            };

            let mut place = Place::new(name, coordinate)
                .with_id(id)
                .with_category(category)
                .with_address(address);

            // Photon reports an extent [min_lon, max_lat, max_lon, min_lat];
            // use its center distance when a reference point is given.
            if let Some(center) = reference {
                place = place.with_distance(coordinate.distance_to(&center));
            }

            // Fill details that Photon already knows about the element.
            place.info = PlaceInfo {
                phone: None,
                website: None,
                opening_hours: None,
                cuisine: None,
                wheelchair: None,
                internet_access: None,
                brand: opt("brand"),
                operator: None,
            };

            places.push(place);
        }
        places
    }
}

impl Default for PhotonProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MapProvider for PhotonProvider {
    fn name(&self) -> &'static str {
        "Photon + CARTO"
    }

    /// Serves the light and dark CARTO basemaps.
    fn supports_style(&self, style: MapStyle) -> bool {
        matches!(style, MapStyle::Light | MapStyle::Dark)
    }

    fn is_available(&self) -> bool {
        self.client()
            .head(&self.photon_url)
            .send()
            .map(|r| r.status().as_u16() < 500)
            .unwrap_or(false)
    }

    fn search(
        &self,
        query: &str,
        near: Option<Coordinate>,
        limit: usize,
    ) -> Result<Vec<Place>, MapsError> {
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("limit", limit.clamp(1, 50).to_string()),
            ("lang", photon_lang()),
        ];
        if let Some(center) = near {
            params.push(("lat", center.latitude.to_string()));
            params.push(("lon", center.longitude.to_string()));
        }
        let json = self.query(&params)?;
        Ok(self.features_to_places(&json, near))
    }

    fn reverse_geocode(&self, coordinate: Coordinate) -> Result<Address, MapsError> {
        let client = self.client();
        let resp = client
            .get(format!("{}/reverse", self.photon_url))
            .query(&[
                ("lat", coordinate.latitude.to_string()),
                ("lon", coordinate.longitude.to_string()),
                ("lang", photon_lang()),
            ])
            .send()?;
        if !resp.status().is_success() {
            return Err(MapsError::Provider(format!(
                "Photon returned status {}",
                resp.status()
            )));
        }
        let json: Value = resp.json()?;
        let feature = json["features"]
            .as_array()
            .and_then(|f| f.first())
            .cloned()
            .ok_or_else(|| MapsError::Parse("Photon reverse returned no feature".into()))?;
        let props = &feature["properties"];

        let opt = |key: &str| {
            props[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let street = opt("street");
        let house_number = opt("housenumber");
        let city = opt("city")
            .or_else(|| opt("district"))
            .or_else(|| opt("locality"));
        let postcode = opt("postcode");
        let country = opt("country");

        let mut parts: Vec<String> = Vec::new();
        if let (Some(ref s), Some(ref n)) = (&street, &house_number) {
            parts.push(format!("{s} {n}"));
        } else if let Some(s) = &street {
            parts.push(s.clone());
        }
        if let Some(pc) = &postcode {
            parts.push(pc.clone());
        }
        if let Some(c) = &city {
            parts.push(c.clone());
        }
        if let Some(c) = &country {
            parts.push(c.clone());
        }

        Ok(Address {
            street,
            house_number,
            postcode,
            city,
            state: opt("state"),
            country,
            country_code: opt("countrycode"),
            formatted: (!parts.is_empty()).then(|| parts.join(", ")),
        })
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

        // Photon has no radius search; query each category keyword biased to
        // the center and keep results within the requested radius.
        let mut all = Vec::new();
        for category in categories {
            let Some(keyword) = category.photon_keyword() else {
                continue;
            };
            let json = self.query(&[
                ("q", keyword.to_string()),
                ("lat", center.latitude.to_string()),
                ("lon", center.longitude.to_string()),
                ("limit", limit.clamp(1, 50).to_string()),
                ("lang", photon_lang()),
            ])?;
            for mut place in self.features_to_places(&json, Some(center)) {
                place.category = *category;
                if let Some(d) = place.distance_m {
                    if d > radius_m as f64 {
                        continue;
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
        all.dedup_by(|a, b| a.id == b.id && a.id.starts_with("osm:"));
        all.truncate(limit);
        Ok(all)
    }

    fn route(
        &self,
        from: Coordinate,
        to: Coordinate,
        mode: TravelMode,
    ) -> Result<Route, MapsError> {
        fetch_osrm_route(
            &self.client(),
            &format!("{}/{}", self.fossgis_url, mode.fossgis_profile()),
            mode.osrm_profile(),
            from,
            to,
            self.name(),
        )
    }

    fn tile_url(&self, x: u32, y: u32, z: u8) -> String {
        format!(
            "https://basemaps.cartocdn.com/{}/{z}/{x}/{y}.png",
            self.carto_slug()
        )
    }
}

/// Maps the system locale onto Photon's `lang` parameter.
fn photon_lang() -> String {
    let lang = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .unwrap_or_default()
        .to_lowercase();
    if lang.starts_with("de") {
        "de".into()
    } else {
        "en".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_urls_follow_style() {
        let light = PhotonProvider::with_config(&MapsConfiguration::new().style(MapStyle::Light));
        assert_eq!(
            light.tile_url(5, 6, 7),
            "https://basemaps.cartocdn.com/rastertiles/voyager/7/5/6.png"
        );

        let dark = PhotonProvider::with_config(&MapsConfiguration::new().style(MapStyle::Dark));
        assert_eq!(
            dark.tile_url(5, 6, 7),
            "https://basemaps.cartocdn.com/dark_all/7/5/6.png"
        );
    }

    #[test]
    fn style_support() {
        let p = PhotonProvider::new();
        assert!(p.supports_style(MapStyle::Light));
        assert!(p.supports_style(MapStyle::Dark));
        assert!(!p.supports_style(MapStyle::Standard));
    }

    #[test]
    fn parses_features() {
        let p = PhotonProvider::new();
        let json = serde_json::json!({
            "features": [{
                "geometry": { "coordinates": [13.4, 52.52] },
                "properties": {
                    "name": "Brandenburger Tor",
                    "osm_key": "historic",
                    "osm_value": "monument",
                    "osm_id": 123,
                    "osm_type": "N",
                    "city": "Berlin",
                    "country": "Germany",
                    "countrycode": "DE"
                }
            }]
        });
        let places = p.features_to_places(&json, Some(Coordinate::new(52.0, 13.0)));
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "Brandenburger Tor");
        assert_eq!(places[0].id, "osm:N123");
        assert!(places[0].distance_m.is_some());
        assert_eq!(places[0].address.city.as_deref(), Some("Berlin"));
    }

    #[test]
    fn lang_detection() {
        std::env::set_var("LANG", "de_DE.UTF-8");
        assert_eq!(photon_lang(), "de");
        std::env::set_var("LANG", "en_US.UTF-8");
        assert_eq!(photon_lang(), "en");
    }
}
