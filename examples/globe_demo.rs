//! Interactive 3D globe demo (GTK4-only).
//!
//! Shows the `GlobeView` with markers for four cities. Drag rotates the
//! earth, scroll zooms, the idle spin can be paused and the camera flies
//! between cities.
//!
//! Run with: `cargo run --example globe_demo`

use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Box as GtkBox, Button, Orientation};
use mapskit::prelude::*;
use std::rc::Rc;

const APP_ID: &str = "org.tontoo.mapskit.globe_demo";

const CITIES: &[(&str, f64, f64)] = &[
    ("Berlin", 52.52, 13.405),
    ("Tokyo", 35.68, 139.69),
    ("New York", 40.71, -74.01),
    ("Sydney", -33.87, 151.21),
];

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        let config = MapsConfiguration::new().style(MapStyle::Light);
        let globe = Rc::new(GlobeView::new(&config));
        globe.set_center(Coordinate::new(52.52, 13.405));
        globe.set_distance(3.0);
        globe.set_auto_rotate(true);

        // Markers for every city.
        globe.set_markers(
            CITIES
                .iter()
                .map(|(name, lat, lon)| GlobeMarker::new(Coordinate::new(*lat, *lon), *name))
                .collect(),
        );

        // ─── Controls ───────────────────────────────────────
        let controls = GtkBox::new(Orientation::Horizontal, 8);

        for (name, lat, lon) in CITIES {
            let button = Button::with_label(name);
            let globe = globe.clone();
            button.connect_clicked(move |_| {
                globe.set_center(Coordinate::new(*lat, *lon));
                globe.set_distance(3.0);
                println!("[globe_demo] flying to {name}");
            });
            controls.append(&button);
        }

        let spin_button = Button::with_label("Pause");
        {
            let globe = globe.clone();
            spin_button.connect_clicked(move |button| {
                let enabled = !globe.auto_rotate();
                globe.set_auto_rotate(enabled);
                button.set_label(if enabled { "Pause" } else { "Spin" });
            });
        }
        controls.append(&spin_button);

        let zoom_in = Button::with_label("Zoom +");
        {
            let globe = globe.clone();
            zoom_in.connect_clicked(move |_| {
                globe.set_distance((globe.distance() - 0.4).max(1.15));
                println!("[globe_demo] distance {:.2}", globe.distance());
            });
        }
        controls.append(&zoom_in);

        let zoom_out = Button::with_label("Zoom -");
        {
            let globe = globe.clone();
            zoom_out.connect_clicked(move |_| {
                globe.set_distance((globe.distance() + 0.5).min(6.0));
            });
        }
        controls.append(&zoom_out);

        let reload = Button::with_label("Reload Texture");
        {
            let globe = globe.clone();
            reload.connect_clicked(move |_| {
                globe.reload_earth_texture();
                println!("[globe_demo] re-downloading earth texture");
            });
        }
        controls.append(&reload);

        // ─── Layout ─────────────────────────────────────────
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.append(&controls);
        root.append(globe.widget());

        let window = ApplicationWindow::builder()
            .application(app)
            .title("TontooMapsKit Globe")
            .default_width(960)
            .default_height(700)
            .build();
        window.set_child(Some(&root));
        window.present();

        println!("[globe_demo] drag rotates, scroll zooms, {} markers shown", CITIES.len());
    });

    app.run()
}
