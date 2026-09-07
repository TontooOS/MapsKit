# Geocoding

Reverse geocoding turns a coordinate into a structured `Address`. Forward
geocoding (address to coordinate) is part of [Search.md](Search.md): search
results carry coordinates and formatted addresses.

## Reverse Geocoding

```rust,no_run
use mapskit::{Coordinate, ProviderChain};

let chain = ProviderChain::default_providers();
let address = chain
    .reverse_geocode(Coordinate::new(52.5163, 13.3777))
    .expect("reverse geocode failed");
println!("{}", address.one_line());
```

The chain tries Nominatim first, then Photon. Both are queried with zoom
18 / full detail so house numbers appear when available.

## The Address Type

| Field | Type | Description |
|---|---|---|
| `street` | `Option<String>` | Street, pedestrian way or footway |
| `house_number` | `Option<String>` | House number |
| `postcode` | `Option<String>` | Postal code |
| `city` | `Option<String>` | City, town, village or municipality |
| `state` | `Option<String>` | State or region |
| `country` | `Option<String>` | Country name |
| `country_code` | `Option<String>` | ISO country code (`DE`) |
| `formatted` | `Option<String>` | Provider-formatted single line |

## One-line Formatting

```rust,no_run
# use mapskit::types::Address;
let address = Address {
    street: Some("Pariser Platz".into()),
    house_number: Some("1".into()),
    postcode: Some("10117".into()),
    city: Some("Berlin".into()),
    ..Default::default()
};
assert_eq!(address.one_line(), "Pariser Platz 1, 10117, Berlin");
```

`one_line()` returns the provider-formatted line when present, otherwise it
joins the available parts with commas in the order street+number, postcode,
city, country.

## Error Behavior

- Returns `MapsError::NoProviderAvailable` when every provider fails; the
  inner error is the last provider error.
- Nominatim answers with an explicit `error` object for coordinates in the
  ocean; this surfaces as `MapsError::Provider`.
- Photon returning no feature is a `MapsError::Parse`.

## Cross References

- [Search.md](Search.md) - forward geocoding via search
- [Providers.md](Providers.md) - which endpoints answer the request
