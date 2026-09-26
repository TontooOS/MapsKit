# TontooMapsKit

Maps framework for TontooOS. Apple MapsKit-style API for TontooUI apps:
an interactive 2D map view, place search with details, forward and reverse
geocoding, turn-by-turn routing and an optional globe view.

Views implement `tontooui::elements::View` and embed into any stack.

See [wiki/MAIN.md](wiki/MAIN.md) for the full documentation.

## Made for TontooOS

Explore more at https://github.com/TontooOS/Libs

## Adding to Your Project

Add to your `Cargo.toml`:

```toml
[dependencies]
sdk = { path = "/Library/System/sdk", features = ["MapsKit"] }
```

## License

TCL v26.1
