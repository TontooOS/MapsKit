//! Places, geocoding and routing services without UI.
//!
//! Demonstrates the provider fallback chain: search with details, reverse
//! geocoding and a short route, all printed to stdout.
//!
//! Run with: `cargo run --example places_search`

use mapskit::prelude::*;

fn main() {
    let chain = ProviderChain::default_providers();
    println!("Providers: {}", chain.names().join(" -> "));

    // ─── Search ─────────────────────────────────────────────
    let results = chain
        .search("Brandenburger Tor Berlin", None, 3)
        .expect("search failed");
    for place in &results {
        println!(
            "Found: {} [{}] at {}",
            place.name,
            format!("{:?}", place.category),
            place.coordinate
        );
        let one_line = place.address.one_line();
        if !one_line.is_empty() {
            println!("       {}", one_line);
        }
    }

    // ─── Place details (enrich the first result) ────────────
    if let Some(first) = results.first() {
        match chain.place_details(first) {
            Ok(details) => {
                if let Some(hours) = &details.info.opening_hours {
                    println!("Opening hours: {hours}");
                }
                if let Some(website) = &details.info.website {
                    println!("Website: {website}");
                }
            }
            Err(e) => eprintln!("details failed: {e}"),
        }
    }

    // ─── Reverse geocoding ──────────────────────────────────
    match chain.reverse_geocode(Coordinate::new(52.5163, 13.3777)) {
        Ok(address) => println!("Reverse: {}", address.one_line()),
        Err(e) => eprintln!("reverse geocode failed: {e}"),
    }

    // ─── Nearby cafes within 800 m ──────────────────────────
    match chain.nearby(
        Coordinate::new(52.5163, 13.3777),
        800,
        &[PlaceCategory::Cafe],
        5,
    ) {
        Ok(cafes) => {
            println!("Cafes nearby:");
            for cafe in &cafes {
                println!(
                    "   {} - {}",
                    cafe.name,
                    cafe.distance_m.map(format_distance).unwrap_or_default()
                );
            }
        }
        Err(e) => eprintln!("nearby failed: {e}"),
    }

    // ─── Routing ────────────────────────────────────────────
    let from = Coordinate::new(52.5163, 13.3777);
    let to = Coordinate::new(52.5219, 13.4132);
    match chain.route(from, to, TravelMode::Walking) {
        Ok(route) => {
            println!(
                "Walk: {} / {} via {}",
                route.distance_text(),
                route.duration_text(),
                route.source
            );
            for step in route.steps.iter().take(6) {
                println!("   - {}", step.instruction);
            }
        }
        Err(e) => eprintln!("route failed: {e}"),
    }
}
