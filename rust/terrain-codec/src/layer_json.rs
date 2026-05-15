//! Cesium `layer.json` (TerrainProvider metadata) builder.
//!
//! `layer.json` is the manifest Cesium fetches before any terrain tiles
//! to learn the format, tiling scheme, attribution, extensions, and
//! per-zoom-level tile availability. This module ships the serde-typed
//! [`LayerJson`] struct, a [`LayerJsonConfig`] builder, and helpers for
//! filling [`TileAvailability`] ranges from geographic bounds (via the
//! schemes in [`crate::tile_coords`]).
//!
//! Reference:
//! <https://github.com/CesiumGS/cesium/wiki/layer.json>

use serde::{Deserialize, Serialize};

use crate::tile_coords::{geodetic_tms, web_mercator, web_mercator_tms};

/// Tiling scheme advertised in `scheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilingScheme {
    /// Global-geodetic TMS (Cesium terrain default).
    Tms,
    /// Web Mercator XYZ.
    Xyz,
}

impl TilingScheme {
    /// Canonical string used in `layer.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tms => "tms",
            Self::Xyz => "xyz",
        }
    }
}

/// Terrain format advertised in `format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainFormat {
    /// Legacy `heightmap-1.0` (u16 grid + child mask + extensions).
    Heightmap1,
    /// `quantized-mesh-1.0` (mesh tiles).
    QuantizedMesh1,
}

impl TerrainFormat {
    /// Canonical string used in `layer.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Heightmap1 => "heightmap-1.0",
            Self::QuantizedMesh1 => "quantized-mesh-1.0",
        }
    }
}

/// One available tile range at a single zoom level.
///
/// `(start_x, start_y)` and `(end_x, end_y)` are inclusive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TileAvailability {
    /// Starting X coordinate (inclusive).
    pub start_x: u32,
    /// Starting Y coordinate (inclusive).
    pub start_y: u32,
    /// Ending X coordinate (inclusive).
    pub end_x: u32,
    /// Ending Y coordinate (inclusive).
    pub end_y: u32,
}

impl TileAvailability {
    /// Create a new range.
    pub const fn new(start_x: u32, start_y: u32, end_x: u32, end_y: u32) -> Self {
        Self {
            start_x,
            start_y,
            end_x,
            end_y,
        }
    }

    /// Full zoom level in the global-geodetic TMS scheme
    /// (`2^(z+1) × 2^z` tiles).
    pub fn full_level_geodetic_tms(zoom: u8) -> Self {
        Self::new(
            0,
            0,
            geodetic_tms::tile_count_x(zoom) - 1,
            geodetic_tms::tile_count_y(zoom) - 1,
        )
    }

    /// Full zoom level in Web Mercator XYZ (`2^z × 2^z` tiles).
    pub fn full_level_xyz(zoom: u8) -> Self {
        let n = web_mercator::tile_count(zoom);
        Self::new(0, 0, n - 1, n - 1)
    }

    /// Range covering `(west, south, east, north)` in the global-geodetic
    /// TMS scheme.
    pub fn from_bounds_geodetic_tms(
        zoom: u8,
        west: f64,
        south: f64,
        east: f64,
        north: f64,
    ) -> Self {
        let (start_x, start_y) = geodetic_tms::lonlat_to_tile(west, south, zoom);
        let (end_x, end_y) = geodetic_tms::lonlat_to_tile(east, north, zoom);
        Self::new(start_x, start_y, end_x, end_y)
    }

    /// Range covering `(west, south, east, north)` in Web Mercator XYZ.
    pub fn from_bounds_xyz(zoom: u8, west: f64, south: f64, east: f64, north: f64) -> Self {
        // XYZ Y=0 is north → south latitude maps to the larger y.
        let (start_x, end_y) = web_mercator::lonlat_to_tile(west, south, zoom);
        let (end_x, start_y) = web_mercator::lonlat_to_tile(east, north, zoom);
        Self::new(start_x, start_y, end_x, end_y)
    }

    /// Range covering `(west, south, east, north)` in the TMS-flipped
    /// Web Mercator scheme (Y=0 south).
    pub fn from_bounds_web_mercator_tms(
        zoom: u8,
        west: f64,
        south: f64,
        east: f64,
        north: f64,
    ) -> Self {
        let (start_x, start_y) = web_mercator_tms::lonlat_to_tile(west, south, zoom);
        let (end_x, end_y) = web_mercator_tms::lonlat_to_tile(east, north, zoom);
        Self::new(start_x, start_y, end_x, end_y)
    }
}

/// Builder-style configuration for assembling a [`LayerJson`].
#[derive(Debug, Clone)]
pub struct LayerJsonConfig {
    /// Tile URL template (e.g. `"{z}/{x}/{y}.terrain"`).
    pub tiles_template: String,
    /// Data version, also used by Cesium for cache busting.
    pub version: String,
    /// Attribution text (HTML allowed).
    pub attribution: Option<String>,
    /// Per-zoom-level tile availability ranges (outer index = zoom level).
    pub available: Vec<Vec<TileAvailability>>,
    /// Minimum zoom level the server supports.
    pub min_zoom: Option<u8>,
    /// Maximum zoom level the server supports.
    pub max_zoom: Option<u8>,
    /// Tiling scheme.
    pub scheme: TilingScheme,
    /// Geographic bounds `[west, south, east, north]`.
    pub bounds: Option<[f64; 4]>,
    /// Enabled extensions (e.g. `"octvertexnormals"`, `"watermask"`,
    /// `"metadata"`).
    pub extensions: Vec<String>,
    /// Terrain format.
    pub format: TerrainFormat,
    /// Metadata-availability level for the `metadata` extension. When
    /// set, Cesium walks the tree by reading each tile's metadata
    /// extension instead of relying on the static `available` array.
    pub metadata_availability: Option<u8>,
}

impl Default for LayerJsonConfig {
    fn default() -> Self {
        Self {
            tiles_template: "{z}/{x}/{y}.terrain".to_string(),
            version: "1.0.0".to_string(),
            attribution: None,
            available: Vec::new(),
            min_zoom: None,
            max_zoom: None,
            scheme: TilingScheme::Tms,
            bounds: None,
            extensions: Vec::new(),
            format: TerrainFormat::QuantizedMesh1,
            metadata_availability: None,
        }
    }
}

/// Serializable Cesium `layer.json` structure.
///
/// Build via [`LayerJson::from_config`] or construct directly. Serialize
/// with `serde_json` to produce the file Cesium expects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LayerJson {
    /// TileJSON version (Cesium expects `"2.1.0"`).
    pub tilejson: String,
    /// Terrain format identifier (e.g. `"quantized-mesh-1.0"`).
    pub format: String,
    /// Data version, used by Cesium for cache busting.
    pub version: String,
    /// Tiling scheme (`"tms"` or `"xyz"`).
    pub scheme: String,
    /// Tile URL templates (Cesium picks the first one).
    pub tiles: Vec<String>,
    /// Per-zoom-level tile availability.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub available: Vec<Vec<TileAvailability>>,
    /// Attribution text.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub attribution: Option<String>,
    /// Minimum zoom level.
    #[serde(skip_serializing_if = "Option::is_none", rename = "minzoom", default)]
    pub min_zoom: Option<u8>,
    /// Maximum zoom level.
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxzoom", default)]
    pub max_zoom: Option<u8>,
    /// Geographic bounds `[west, south, east, north]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bounds: Option<[f64; 4]>,
    /// Enabled extensions.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub extensions: Vec<String>,
    /// Metadata-availability zoom depth for the `metadata` extension.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata_availability: Option<u8>,
}

impl LayerJson {
    /// Build from a [`LayerJsonConfig`].
    pub fn from_config(config: &LayerJsonConfig) -> Self {
        Self {
            tilejson: "2.1.0".to_string(),
            format: config.format.as_str().to_string(),
            version: config.version.clone(),
            scheme: config.scheme.as_str().to_string(),
            tiles: vec![config.tiles_template.clone()],
            available: config.available.clone(),
            attribution: config.attribution.clone(),
            min_zoom: config.min_zoom,
            max_zoom: config.max_zoom,
            bounds: config.bounds,
            extensions: config.extensions.clone(),
            metadata_availability: config.metadata_availability,
        }
    }

    /// Convenience: serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_level_geodetic_tms_z0_covers_two_x_one_y() {
        let r = TileAvailability::full_level_geodetic_tms(0);
        assert_eq!(r, TileAvailability::new(0, 0, 1, 0));
    }

    #[test]
    fn from_bounds_geodetic_tms_is_ordered() {
        let r = TileAvailability::from_bounds_geodetic_tms(4, 122.0, 20.0, 154.0, 46.0);
        assert!(r.start_x <= r.end_x);
        assert!(r.start_y <= r.end_y);
    }

    #[test]
    fn layer_json_round_trips_through_serde() {
        let cfg = LayerJsonConfig {
            attribution: Some("Made with terrain-codec".into()),
            available: vec![vec![TileAvailability::full_level_geodetic_tms(0)]],
            min_zoom: Some(0),
            max_zoom: Some(10),
            bounds: Some([-180.0, -90.0, 180.0, 90.0]),
            extensions: vec!["octvertexnormals".into(), "watermask".into()],
            format: TerrainFormat::QuantizedMesh1,
            metadata_availability: Some(10),
            ..Default::default()
        };
        let lj = LayerJson::from_config(&cfg);
        let json = serde_json::to_string(&lj).unwrap();
        let parsed: LayerJson = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, lj);
        assert!(json.contains("quantized-mesh-1.0"));
        assert!(json.contains("octvertexnormals"));
        // The `metadata_availability` field uses camelCase.
        assert!(json.contains("metadataAvailability"));
    }

    #[test]
    fn empty_optionals_are_omitted() {
        let lj = LayerJson::from_config(&LayerJsonConfig::default());
        let json = serde_json::to_string(&lj).unwrap();
        assert!(!json.contains("available"));
        assert!(!json.contains("bounds"));
        assert!(!json.contains("attribution"));
        assert!(!json.contains("extensions"));
        assert!(!json.contains("minzoom"));
        assert!(!json.contains("maxzoom"));
    }
}
