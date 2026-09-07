# FFI

MapsKit exposes a C API from the `cdylib` for non-Rust consumers. The
matching header lives at `Headers/mapskit.h`. Configuration is passed as a
JSON string; service results are returned as JSON strings owned by the
caller.

## Linking

Link against `/Library/System/mapskit.library` and include the header:

```c
#include "mapskit.h"

const char *version = tontoo_mapskit_version(); /* "26.1.0" */
```

## Services (blocking)

```c
char *error = NULL;

/* Search with a reference coordinate bias. */
char *json = tontoo_mapskit_search("Brandenburger Tor",
                                   52.52, 13.405, 1, 5, &error);
if (!json) {
    fprintf(stderr, "search failed: %s\n", error ? error : "?");
    tontoo_mapskit_string_free(error);
}
/* json: [{ id, name, category, latitude, longitude, address,
            phone, website, opening_hours, distance_m }, ...] */
tontoo_mapskit_string_free(json);

/* Reverse geocode. */
char *address = tontoo_mapskit_reverse_geocode(52.5163, 13.3777, &error);

/* Route; mode 0 = driving, 1 = walking, 2 = cycling. */
char *route = tontoo_mapskit_route(52.5163, 13.3777,
                                   52.5219, 13.4132, 0, &error);
```

| Function | Returns | JSON shape |
|---|---|---|
| `tontoo_mapskit_search` | Array of places | `{id, name, category, latitude, longitude, address, phone, website, opening_hours, distance_m}` |
| `tontoo_mapskit_reverse_geocode` | Address object | `{street, house_number, postcode, city, state, country, formatted}` |
| `tontoo_mapskit_route` | Route object | `{source, distance_m, duration_s, geometry, steps}` |

All three block on network I/O - call them from worker threads. On failure
they return `NULL` and write a message to `error_out`.

## 2D Map View

```c
TontooMapView *view = tontoo_mapskit_view_new(
    "{\"style\": \"dark\", \"shows_points_of_interest\": true}", NULL);
GtkWidget *widget = tontoo_mapskit_view_widget(view); /* borrowed */

tontoo_mapskit_view_set_center(view, 52.52, 13.405, 12.0);
tontoo_mapskit_view_add_annotation(view,
    "{\"latitude\": 52.52, \"longitude\": 13.405, \"title\": \"Berlin\"}");
tontoo_mapskit_view_show_user_location(view);

tontoo_mapskit_view_free(view);
```

Config keys: `style` (`light`, `dark`, `standard`),
`shows_points_of_interest`, `cache_directory`, `user_agent`,
`globe_earth_texture`. Passing `NULL` uses defaults. The view initializes
GTK itself if the host has not done so yet.

## 3D Globe View

```c
TontooGlobe *globe = tontoo_mapskit_globe_new("{\"style\": \"dark\"}", NULL);
GtkWidget *widget = tontoo_mapskit_globe_widget(globe); /* borrowed */

tontoo_mapskit_globe_set_center(globe, 48.137, 11.575);
tontoo_mapskit_globe_add_marker(globe, 48.137, 11.575, "Munich");
tontoo_mapskit_globe_set_auto_rotate(globe, 1);

tontoo_mapskit_globe_free(globe);
```

## Return Codes

Numeric-returning functions follow this convention:

| Return | Meaning |
|---|---|
| pointer | Handle or JSON string; `NULL` means failure (see `error_out`) |
| `void` setters | Cannot fail; invalid input is ignored |

## Memory Rules

| Rule | Detail |
|---|---|
| Strings | Free every returned string and every `*error_out` with `tontoo_mapskit_string_free` |
| Widgets | Borrowed from their handle; never free directly |
| Handles | Free exactly once with `..._view_free` / `..._globe_free`; do not use afterwards |
| Config JSON | Owned by the caller; only read during the call |

## Cross References

- [MapView.md](MapView.md) / [GlobeView.md](GlobeView.md) - behavior behind
  the handles
- [Providers.md](Providers.md) - services exposed over FFI
- [UIKit.md](UIKit.md) - the Rust-side equivalent embedding API
