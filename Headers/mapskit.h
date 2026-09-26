/*
 * TontooMapsKit - C API
 *
 * Maps framework for TontooOS: interactive 2D map views, place search,
 * geocoding, routing and an optional globe view. Two providers are built
 * in with automatic fallback (OpenStreetMap primary; Photon + OSM/Esri
 * fallback). No API keys required.
 *
 * Views render inside TontooUI from Rust. The view handles below are
 * opaque *model* objects (camera, annotations, markers) - no widget
 * pointers are handed out. Embed the Rust `MapView` / `GlobeView`
 * (which implement `tontooui::elements::View`) in your TontooUI shell
 * and drive the shared model from C through these handles.
 *
 * All strings returned by this library are owned by the caller and must be
 * freed with tontoo_mapskit_string_free().
 */

#ifndef TONTOO_MAPSKIT_H
#define TONTOO_MAPSKIT_H

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Version                                                             */
/* ------------------------------------------------------------------ */

/* The framework version string, e.g. "26.1.0". Static, do not free. */
const char *tontoo_mapskit_version(void);

/* ------------------------------------------------------------------ */
/* Services (blocking - call from worker threads)                      */
/* ------------------------------------------------------------------ */

/*
 * Search places and addresses.
 *
 * query         NUL-terminated search text.
 * ref_lat/lon   Reference coordinate used to rank results (bias).
 * has_reference Nonzero to enable the reference bias.
 * limit         Maximum number of results (1..50).
 * error_out     Receives an error message on failure; may be NULL.
 *
 * Returns a JSON array of places or NULL on error:
 * [{ id, name, category, latitude, longitude, address, phone,
 *    website, opening_hours, distance_m }, ...]
 */
char *tontoo_mapskit_search(const char *query,
                            double ref_lat,
                            double ref_lon,
                            int has_reference,
                            int limit,
                            char **error_out);

/*
 * Reverse geocode a coordinate into a JSON address object or NULL:
 * { street, house_number, postcode, city, state, country, formatted }
 */
char *tontoo_mapskit_reverse_geocode(double lat,
                                     double lon,
                                     char **error_out);

/*
 * Compute a route between two coordinates.
 *
 * mode  0 = driving, 1 = walking, 2 = cycling.
 *
 * Returns a JSON route object or NULL on error:
 * { source, distance_m, duration_s,
 *   geometry: [[lon, lat], ...],
 *   steps: [{ instruction, distance_m, duration_s, latitude, longitude }] }
 */
char *tontoo_mapskit_route(double from_lat,
                           double from_lon,
                           double to_lat,
                           double to_lon,
                           int mode,
                           char **error_out);

/* ------------------------------------------------------------------ */
/* 2D map model (rendered inside TontooUI from Rust)                   */
/* ------------------------------------------------------------------ */

typedef struct _TontooMapView TontooMapView;

/*
 * Create a 2D map model. config_json is a JSON object with optional keys:
 * { "style": "light" | "dark" | "standard" | "satellite",
 *   "shows_points_of_interest": true,
 *   "cache_directory": "/path",
 *   "user_agent": "MyApp/1.0",
 *   "backend_url": "http://192.168.1.100/tontooos/api/maps" }
 * May be NULL for defaults.
 */
TontooMapView *tontoo_mapskit_view_new(const char *config_json);

/* Set the camera center and zoom level (0..19). */
void tontoo_mapskit_view_set_center(TontooMapView *view,
                                    double lat,
                                    double lon,
                                    double zoom);

/* Add a pin: JSON with latitude, longitude, title [, subtitle]. */
void tontoo_mapskit_view_add_annotation(TontooMapView *view,
                                        const char *annotation_json);

/* Display a route JSON produced by tontoo_mapskit_route() and fit it. */
void tontoo_mapskit_view_display_route(TontooMapView *view,
                                       const char *route_json);

/* Show the blue user location dot (uses CoreLocation) and center on it. */
void tontoo_mapskit_view_show_user_location(TontooMapView *view);

/* Destroy the map model handle. */
void tontoo_mapskit_view_free(TontooMapView *view);

/* ------------------------------------------------------------------ */
/* Globe model (rendered inside TontooUI from Rust)                    */
/* ------------------------------------------------------------------ */

typedef struct _TontooGlobe TontooGlobe;

/* Create a globe model (same config keys as the map model). */
TontooGlobe *tontoo_mapskit_globe_new(const char *config_json);

/* Center the globe on a coordinate. */
void tontoo_mapskit_globe_set_center(TontooGlobe *globe,
                                     double lat,
                                     double lon);

/* Add an accent colored marker dot with an optional label. */
void tontoo_mapskit_globe_add_marker(TontooGlobe *globe,
                                     double lat,
                                     double lon,
                                     const char *label);

/* Enable (nonzero) or disable the idle rotation. */
void tontoo_mapskit_globe_set_auto_rotate(TontooGlobe *globe,
                                          int enabled);

/* Destroy the globe model handle. */
void tontoo_mapskit_globe_free(TontooGlobe *globe);

/* ------------------------------------------------------------------ */
/* Memory                                                              */
/* ------------------------------------------------------------------ */

/* Free any string returned by this library or written to error_out. */
void tontoo_mapskit_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* TONTOO_MAPSKIT_H */
