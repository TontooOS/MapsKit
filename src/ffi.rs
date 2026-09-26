//! C FFI exposed from the `cdylib`. The matching header lives at
//! `Headers/mapskit.h`.
//!
//! The FFI layer lets C / C++ apps (and other languages that can load a
//! shared library) drive the TontooMapsKit place/routing services and the
//! map/globe models. Views render inside TontooUI from Rust, so no widget
//! pointers are handed out: view handles are opaque model objects (camera, annotations, markers) that the host embeds
//! through its own TontooUI shell.
//!
//! Configuration is passed as a JSON string; results are returned as JSON
//! strings owned by the caller until `tontoo_mapskit_string_free` is called.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::sync::OnceLock;

use crate::camera::MapCamera;
use crate::config::{MapStyle, MapsConfiguration};
use crate::globe_view::{GlobeMarker, GlobeView};
use crate::map_view::MapView;
use crate::overlays::Annotation;
use crate::providers::ProviderChain;
use crate::types::{Coordinate, Place, Route, TravelMode};

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

fn shared_chain() -> &'static ProviderChain {
    static CHAIN: OnceLock<ProviderChain> = OnceLock::new();
    CHAIN.get_or_init(ProviderChain::default_providers)
}

fn set_error(out: *mut *mut c_char, message: &str) {
    if !out.is_null() {
        unsafe {
            *out = CString::new(message)
                .map(|c| c.into_raw())
                .unwrap_or(std::ptr::null_mut());
        }
    }
}

fn json_ptr(value: &serde_json::Value) -> *mut c_char {
    CString::new(value.to_string())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn read_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr).to_str().ok().map(str::to_string) }
}

fn config_from_json(json: &str) -> MapsConfiguration {
    let mut config = MapsConfiguration::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return config;
    };
    match value["style"].as_str() {
        Some("dark") => config.style = MapStyle::Dark,
        Some("standard") => config.style = MapStyle::Standard,
        Some("satellite") => config.style = MapStyle::Satellite,
        _ => config.style = MapStyle::Light,
    }
    if let Some(show) = value["shows_points_of_interest"].as_bool() {
        config.shows_points_of_interest = show;
    }
    if let Some(dir) = value["cache_directory"].as_str() {
        config.cache_directory = Some(dir.into());
    }
    if let Some(agent) = value["user_agent"].as_str() {
        config.user_agent = agent.to_string();
    }
    if let Some(texture) = value["globe_earth_texture"].as_bool() {
        config.globe_earth_texture = texture;
    }
    config
}

fn travel_mode(code: c_int) -> TravelMode {
    match code {
        1 => TravelMode::Walking,
        2 => TravelMode::Cycling,
        _ => TravelMode::Driving,
    }
}

fn place_to_json(place: &Place) -> serde_json::Value {
    serde_json::json!({
        "id": place.id,
        "name": place.name,
        "category": format!("{:?}", place.category),
        "latitude": place.coordinate.latitude,
        "longitude": place.coordinate.longitude,
        "address": place.address.one_line(),
        "phone": place.info.phone,
        "website": place.info.website,
        "opening_hours": place.info.opening_hours,
        "distance_m": place.distance_m,
    })
}

fn route_to_json(route: &Route) -> serde_json::Value {
    serde_json::json!({
        "source": route.source,
        "distance_m": route.distance_m,
        "duration_s": route.duration_s,
        "geometry": route.geometry.iter()
            .map(|c| [c.longitude, c.latitude])
            .collect::<Vec<_>>(),
        "steps": route.steps.iter().map(|s| serde_json::json!({
            "instruction": s.instruction,
            "distance_m": s.distance_m,
            "duration_s": s.duration_s,
            "latitude": s.coordinate.latitude,
            "longitude": s.coordinate.longitude,
        })).collect::<Vec<_>>(),
    })
}

// ═══════════════════════════════════════════════════════════════
// Service functions
// ═══════════════════════════════════════════════════════════════

/// The framework version as a static C string.
#[no_mangle]
pub extern "C" fn tontoo_mapskit_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Search for places. Returns a JSON array or null (see `error_out`).
///
/// # Safety
///
/// `query` must be NUL-terminated. `error_out`, when not null, must point to
/// a writable `char*`. Blocking - call from a worker thread.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_search(
    query: *const c_char,
    ref_lat: c_double,
    ref_lon: c_double,
    has_reference: c_int,
    limit: c_int,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(query) = read_str(query) else {
        set_error(error_out, "query is null");
        return std::ptr::null_mut();
    };
    let near = (has_reference != 0).then(|| Coordinate::new(ref_lat, ref_lon));
    match shared_chain().search(&query, near, limit.clamp(1, 50) as usize) {
        Ok(places) => {
            let list: Vec<_> = places.iter().map(place_to_json).collect();
            json_ptr(&serde_json::Value::Array(list))
        }
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Reverse geocode a coordinate. Returns a JSON object or null.
///
/// # Safety
///
/// `error_out` must be valid when not null. Blocking - worker thread only.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_reverse_geocode(
    lat: c_double,
    lon: c_double,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    match shared_chain().reverse_geocode(Coordinate::new(lat, lon)) {
        Ok(address) => json_ptr(&serde_json::json!({
            "street": address.street,
            "house_number": address.house_number,
            "postcode": address.postcode,
            "city": address.city,
            "state": address.state,
            "country": address.country,
            "formatted": address.one_line(),
        })),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Compute a route. `mode`: 0 driving, 1 walking, 2 cycling.
/// Returns a JSON object or null.
///
/// # Safety
///
/// `error_out` must be valid when not null. Blocking - worker thread only.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_route(
    from_lat: c_double,
    from_lon: c_double,
    to_lat: c_double,
    to_lon: c_double,
    mode: c_int,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let from = Coordinate::new(from_lat, from_lon);
    let to = Coordinate::new(to_lat, to_lon);
    match shared_chain().route(from, to, travel_mode(mode)) {
        Ok(route) => json_ptr(&route_to_json(&route)),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Map model
// ═══════════════════════════════════════════════════════════════

/// Opaque map model handle handed to the C caller.
///
/// The model holds the camera, annotations and overlays of a `MapView`.
/// Rendering happens inside TontooUI from Rust; this handle only drives the
/// model.
pub struct TontooMapView {
    view: MapView,
}

/// Create a 2D map model from a JSON configuration string.
///
/// Config keys: `style` (`light`/`dark`/`standard`/`satellite`),
/// `shows_points_of_interest`, `cache_directory`, `user_agent`.
///
/// # Safety
///
/// `config_json` must be NUL-terminated or null for defaults.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_new(
    config_json: *const c_char,
) -> *mut TontooMapView {
    let config = read_str(config_json)
        .map(|json| config_from_json(&json))
        .unwrap_or_default();
    Box::into_raw(Box::new(TontooMapView {
        view: MapView::new(&config),
    }))
}

/// Set the camera center and zoom level.
///
/// # Safety
///
/// `view` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_set_center(
    view: *mut TontooMapView,
    lat: c_double,
    lon: c_double,
    zoom: c_double,
) {
    (*view).view.set_camera(MapCamera::new(Coordinate::new(lat, lon), zoom));
}

/// Add an annotation pin. JSON keys: `latitude`, `longitude`, `title`,
/// optional `subtitle`.
///
/// # Safety
///
/// `view` must be valid; `annotation_json` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_add_annotation(
    view: *mut TontooMapView,
    annotation_json: *const c_char,
) {
    let Some(json) = read_str(annotation_json) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };
    let Some((lat, lon)) = value["latitude"].as_f64().zip(value["longitude"].as_f64()) else {
        return;
    };
    let mut annotation =
        Annotation::new(Coordinate::new(lat, lon), value["title"].as_str().unwrap_or(""));
    if let Some(subtitle) = value["subtitle"].as_str() {
        annotation = annotation.subtitle(subtitle);
    }
    (*view).view.add_annotation(annotation);
}

/// Display a previously computed route JSON (see `tontoo_mapskit_route`).
///
/// # Safety
///
/// `view` and `route_json` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_display_route(
    view: *mut TontooMapView,
    route_json: *const c_char,
) {
    let Some(json) = read_str(route_json) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };
    let geometry: Vec<Coordinate> = value["geometry"]
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|p| {
                    Some(Coordinate::new(p[1].as_f64()?, p[0].as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let steps = Vec::new();
    let route = Route {
        distance_m: value["distance_m"].as_f64().unwrap_or(0.0),
        duration_s: value["duration_s"].as_f64().unwrap_or(0.0),
        geometry,
        steps,
        source: String::new(),
    };
    (*view).view.display_route(&route);
}

/// Show the blue user location dot using CoreLocation.
///
/// Resolves on a worker thread and centers the model when it answers.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_show_user_location(view: *mut TontooMapView) {
    (*view).view.show_user_location();
}

/// Destroy a map model handle.
///
/// # Safety
///
/// `view` must not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_view_free(view: *mut TontooMapView) {
    if !view.is_null() {
        drop(Box::from_raw(view));
    }
}

// ═══════════════════════════════════════════════════════════════
// Globe model
// ═══════════════════════════════════════════════════════════════

/// Opaque globe model handle handed to the C caller.
///
/// Rendering happens inside TontooUI from Rust; this handle only drives the
/// model (center, markers, rotation).
pub struct TontooGlobe {
    globe: GlobeView,
}

/// Create a globe model from a JSON configuration string.
///
/// # Safety
///
/// `config_json` must be NUL-terminated or null for defaults.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_globe_new(
    config_json: *const c_char,
) -> *mut TontooGlobe {
    let config = read_str(config_json)
        .map(|json| config_from_json(&json))
        .unwrap_or_default();
    Box::into_raw(Box::new(TontooGlobe {
        globe: GlobeView::new(&config),
    }))
}

/// Center the globe on a coordinate.
///
/// # Safety
///
/// `globe` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_globe_set_center(
    globe: *mut TontooGlobe,
    lat: c_double,
    lon: c_double,
) {
    (*globe).globe.set_center(Coordinate::new(lat, lon));
}

/// Add an accent marker dot on the globe.
///
/// # Safety
///
/// `globe` must be valid; `label` may be null.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_globe_add_marker(
    globe: *mut TontooGlobe,
    lat: c_double,
    lon: c_double,
    label: *const c_char,
) {
    let label = read_str(label).unwrap_or_default();
    (*globe)
        .globe
        .add_marker(GlobeMarker::new(Coordinate::new(lat, lon), label));
}

/// Enable or disable the idle rotation (nonzero = on).
///
/// # Safety
///
/// `globe` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_globe_set_auto_rotate(
    globe: *mut TontooGlobe,
    enabled: c_int,
) {
    (*globe).globe.set_auto_rotate(enabled != 0);
}

/// Destroy a globe model handle.
///
/// # Safety
///
/// `globe` must not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_globe_free(globe: *mut TontooGlobe) {
    if !globe.is_null() {
        drop(Box::from_raw(globe));
    }
}

/// Free a string returned by this library.
///
/// # Safety
///
/// `s` must be a pointer returned by this API or written to `error_out`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_mapskit_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
