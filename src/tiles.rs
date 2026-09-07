//! Slippy map tile math, fetching and on-disk caching.
//!
//! Tiles follow the standard `z/x/y` scheme: zoom level `z` has `2^z` tiles
//! per axis, tile `(0,0)` is the top-left corner of the Web Mercator plane.

use crate::config::MapsConfiguration;
use crate::error::MapsError;
use crate::providers::MapProvider;
use crate::types::Coordinate;
use std::io::Write;
use std::path::PathBuf;

/// Maximum supported zoom level.
pub const MAX_ZOOM: u8 = 19;

/// Identifies a single map tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileKey {
    pub fn new(z: u8, x: u32, y: u32) -> Self {
        Self { z, x, y }
    }

    /// The coordinate of the tile's north-west corner.
    pub fn north_west(&self) -> Coordinate {
        coord_for_tile(self.x, self.y, self.z)
    }

    /// The coordinate of the tile's center.
    pub fn center(&self) -> Coordinate {
        coord_for_tile_f(self.x as f64 + 0.5, self.y as f64 + 0.5, self.z)
    }

    /// The four neighbors of this tile; `None` outside the world.
    pub fn neighbors(&self) -> [Option<TileKey>; 4] {
        let n = (1u32 << self.z) - 1;
        [
            (self.x > 0).then(|| TileKey::new(self.z, self.x - 1, self.y)),
            (self.x < n).then(|| TileKey::new(self.z, self.x + 1, self.y)),
            (self.y > 0).then(|| TileKey::new(self.z, self.x, self.y - 1)),
            (self.y < n).then(|| TileKey::new(self.z, self.x, self.y + 1)),
        ]
    }
}

/// Clamps a zoom level into `[0, MAX_ZOOM]`.
pub fn clamp_zoom(zoom: f64) -> u8 {
    zoom.round().clamp(0.0, MAX_ZOOM as f64) as u8
}

/// Converts a coordinate to fractional tile coordinates at zoom `z`.
pub fn tile_for(coordinate: Coordinate, z: u8) -> (f64, f64) {
    let n = f64::from(1u32 << z);
    let clamped_lat = coordinate.latitude.clamp(-85.051_128_78, 85.051_128_78);
    let lat_rad = clamped_lat.to_radians();
    let x = (coordinate.longitude + 180.0) / 360.0 * n;
    let merc = (lat_rad.tan() + 1.0 / lat_rad.cos()).ln();
    let y = (1.0 - merc / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// Integer tile containing the coordinate at zoom `z`.
pub fn tile_containing(coordinate: Coordinate, z: u8) -> TileKey {
    let (fx, fy) = tile_for(coordinate, z);
    TileKey::new(z, fx.floor() as u32, fy.floor() as u32)
}

/// North-west corner coordinate of an integer tile.
pub fn coord_for_tile(x: u32, y: u32, z: u8) -> Coordinate {
    coord_for_tile_f(f64::from(x), f64::from(y), z)
}

fn coord_for_tile_f(x: f64, y: f64, z: u8) -> Coordinate {
    let n = f64::from(1u32 << z);
    let longitude = x / n * 360.0 - 180.0;
    let latitude = ((std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh()).atan().to_degrees();
    Coordinate::new(latitude, longitude)
}

/// Meters per pixel at a given zoom and latitude (256 px tiles).
pub fn meters_per_pixel(zoom: u8, latitude: f64) -> f64 {
    156_543.03392 * latitude.to_radians().cos() / f64::from(1u32 << zoom)
}

/// On-disk tile cache following the provider name and style.
pub struct TileCache {
    root: PathBuf,
}

impl TileCache {
    /// Creates the cache from the configuration, creating directories.
    ///
    /// `source_key` separates variants of one provider that serve different
    /// imagery per style (e.g. CARTO Voyager vs Dark Matter), so switching
    /// styles never reads tiles of another style. Use
    /// [`cache_source_key`] to derive it from a provider.
    pub fn new(config: &MapsConfiguration, provider_name: &str, source_key: &str) -> Self {
        let sanitize = |text: &str| -> String {
            text.chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect()
        };
        let root = config
            .resolved_cache_dir()
            .join("tiles")
            .join(format!("{}-{}", sanitize(provider_name), sanitize(source_key))
                .to_lowercase());
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    fn path(&self, key: &TileKey) -> PathBuf {
        self.root
            .join(key.z.to_string())
            .join(key.x.to_string())
            .join(format!("{}.png", key.y))
    }

    /// Returns cached PNG bytes when present.
    pub fn get(&self, key: &TileKey) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    /// Stores PNG bytes in the cache.
    pub fn put(&self, key: &TileKey, bytes: &[u8]) {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::File::create(&path) {
            let _ = file.write_all(bytes);
        }
    }
}

/// Stable cache namespace for a tile source: provider name plus a hash of
/// a sample tile URL. Two style variants that return different URLs (e.g.
/// CARTO Voyager and Dark Matter) therefore never share one directory.
pub fn cache_source_key(provider: &dyn MapProvider) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    provider.tile_url(0, 0, 0).hash(&mut hasher);
    format!("{}-{:08x}", provider.name(), hasher.finish() as u32)
}

/// Whether the bytes start with a supported image magic number. Raster map
/// tiles arrive as PNG (OSM, CARTO) or JPEG (Esri World Imagery satellite).
fn is_supported_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// Fetches one tile through the provider with disk caching.
///
/// Blocking — call from a worker thread.
pub fn fetch_tile(
    provider: &dyn MapProvider,
    cache: &TileCache,
    key: TileKey,
    user_agent: &str,
    timeout_seconds: u64,
) -> Result<Vec<u8>, MapsError> {
    if let Some(bytes) = cache.get(&key) {
        return Ok(bytes);
    }

    let url = provider.tile_url(key.x, key.y, key.z);
    let client = reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .timeout(std::time::Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|e| MapsError::Network(e.to_string()))?;

    let resp = client.get(url).send()?;
    if !resp.status().is_success() {
        return Err(MapsError::Tile(format!(
            "{} returned status {} for {}/{}/{}",
            provider.name(),
            resp.status(),
            key.z,
            key.x,
            key.y
        )));
    }
    let bytes = resp.bytes()?.to_vec();

    // Basic sanity check: PNG (OSM/CARTO) or JPEG (Esri satellite) magic.
    if !is_supported_image(&bytes) {
        return Err(MapsError::Tile(
            "response is not a PNG or JPEG image".into(),
        ));
    }

    cache.put(&key, &bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_tile_math() {
        // Berlin at zoom 3 is tile (4, 2)-ish; verify exact known values.
        let berlin = Coordinate::new(52.52, 13.405);
        let key = tile_containing(berlin, 3);
        assert_eq!(key, TileKey::new(3, 4, 2));

        // Roundtrip through the tile center lands in the same tile.
        let center = key.center();
        assert_eq!(tile_containing(center, 3), key);
    }

    #[test]
    fn zero_zoom_is_whole_world() {
        let key = tile_containing(Coordinate::new(-33.9, 151.2), 0);
        assert_eq!(key, TileKey::new(0, 0, 0));
    }

    #[test]
    fn neighbors_respect_world_bounds() {
        let key = TileKey::new(1, 0, 0);
        let n = key.neighbors();
        assert!(n[0].is_none(), "west neighbor outside world");
        assert!(n[2].is_none(), "north neighbor outside world");
        assert_eq!(n[1], Some(TileKey::new(1, 1, 0)));
        assert_eq!(n[3], Some(TileKey::new(1, 0, 1)));
    }

    #[test]
    fn zoom_clamping() {
        assert_eq!(clamp_zoom(-3.0), 0);
        assert_eq!(clamp_zoom(7.6), 8);
        assert_eq!(clamp_zoom(99.0), MAX_ZOOM);
    }

    #[test]
    fn meters_per_pixel_shrinks_with_zoom() {
        assert!(meters_per_pixel(10, 0.0) > meters_per_pixel(15, 0.0));
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mapskit-test-{}", std::process::id()));
        let config = MapsConfiguration::new().cache_directory(dir.clone());
        let cache = TileCache::new(&config, "Test Provider", "voyager");
        let key = TileKey::new(5, 3, 7);
        assert!(cache.get(&key).is_none());
        cache.put(&key, &[0x89, b'P', b'N', b'G']);
        assert_eq!(cache.get(&key), Some(vec![0x89, b'P', b'N', b'G']));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn source_keys_differ_per_style() {
        use crate::providers::photon::PhotonProvider;
        let light =
            PhotonProvider::with_config(&MapsConfiguration::new().style(crate::config::MapStyle::Light));
        let dark =
            PhotonProvider::with_config(&MapsConfiguration::new().style(crate::config::MapStyle::Dark));
        assert_ne!(
            cache_source_key(&light),
            cache_source_key(&dark)
        );
    }

    #[test]
    fn image_magic_numbers() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0];
        let garbage = [b'<', b'h', b't', b'm', b'l'];
        assert!(is_supported_image(&png));
        assert!(is_supported_image(&jpeg));
        assert!(!is_supported_image(&garbage));
        assert!(!is_supported_image(&[]));
    }
}
