//! Interactive TontooMapsKit demo (GTK4-only).
//!
//! A map with a search bar, a route demo button and a user location button.
//! Search and routing run on worker threads so the UI stays responsive.
//!
//! Run with: `cargo run --example maps_demo`

use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Box as GtkBox, Button, Entry, Orientation};
use mapskit::prelude::*;
use std::rc::Rc;

const APP_ID: &str = "org.tontoo.mapskit.demo";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        let config = MapsConfiguration::new()
            .style(MapStyle::Light)
            .shows_points_of_interest(true);

        let map = Rc::new(MapView::new(&config));
        map.set_center(Coordinate::new(52.5163, 13.3777), 13.0);

        // ─── Search bar ─────────────────────────────────────
        let search = Entry::builder()
            .placeholder_text(mapskit::localization::t_or(
                "mapskit.search.placeholder",
                "Search places and addresses",
            ))
            .hexpand(true)
            .build();

        {
            let map = map.clone();
            search.connect_activate(move |entry| {
                let query = entry.text().to_string();
                if query.trim().is_empty() {
                    return;
                }
                let center = map.camera().center;
                let map = map.clone();
                let (sender, receiver) =
                    async_channel::unbounded::<Result<Vec<Place>, MapsError>>();

                std::thread::spawn(move || {
                    let result =
                        ProviderChain::default_providers().search(&query, Some(center), 10);
                    let _ = sender.send_blocking(result);
                });

                glib::spawn_future_local(async move {
                    // Outer Ok = channel receive, inner Ok = service result.
                    if let Ok(Ok(places)) = receiver.recv().await {
                        map.clear_annotations();
                        if places.is_empty() {
                            println!(
                                "{}",
                                mapskit::localization::t_or(
                                    "mapskit.search.no_results",
                                    "No results found."
                                )
                            );
                            return;
                        }
                        for place in &places {
                            println!("  {} - {}", place.name, place.address.one_line());
                        }
                        if let Some(first) = places.first() {
                            map.show_place(first);
                        }
                        for place in &places[1..] {
                            map.add_place(place);
                        }
                    }
                });
            });
        }

        // ─── Route demo: Brandenburg Gate -> Alexanderplatz ──
        let route_button = Button::with_label("Route");
        {
            let map = map.clone();
            route_button.connect_clicked(move |_| {
                let from = Coordinate::new(52.5163, 13.3777); // Brandenburg Gate
                let to = Coordinate::new(52.5219, 13.4132); // Alexanderplatz
                let map = map.clone();
                let (sender, receiver) = async_channel::unbounded::<Result<Route, MapsError>>();

                std::thread::spawn(move || {
                    let result =
                        ProviderChain::default_providers().route(from, to, TravelMode::Driving);
                    let _ = sender.send_blocking(result);
                });

                glib::spawn_future_local(async move {
                    if let Ok(Ok(route)) = receiver.recv().await {
                        println!(
                            "[maps_demo] route {} / {} via {}",
                            route.distance_text(),
                            route.duration_text(),
                            route.source
                        );
                        for step in route.steps.iter().take(5) {
                            println!("   - {}", step.instruction);
                        }
                        map.display_route(&route);
                    }
                });
            });
        }

        // ─── User location ──────────────────────────────────
        let locate_button = Button::with_label("Locate");
        {
            let map = map.clone();
            locate_button.connect_clicked(move |_| {
                map.show_user_location();
            });
        }

        // ─── Annotation taps print the place name ───────────
        {
            let tapped_map = map.clone();
            map.on_annotation_tapped(move |annotation| {
                let _ = &tapped_map;
                println!("[maps_demo] tapped: {}", annotation.title);
            });
        }

        // ─── Layout ─────────────────────────────────────────
        let controls = GtkBox::new(Orientation::Horizontal, 8);
        controls.append(&search);
        controls.append(&route_button);
        controls.append(&locate_button);

        let root = GtkBox::new(Orientation::Vertical, 0);
        root.append(&controls);
        root.append(map.widget());

        let window = ApplicationWindow::builder()
            .application(app)
            .title("TontooMapsKit")
            .default_width(1000)
            .default_height(700)
            .build();
        window.set_child(Some(&root));
        window.present();
    });

    app.run()
}
