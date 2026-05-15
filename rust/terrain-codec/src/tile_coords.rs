//! Tile coordinate utilities for the tiling schemes commonly used with
//! Cesium terrain and Web Mercator raster sources.
//!
//! Three schemes are supported, each in its own submodule:
//!
//! - [`web_mercator`] — XYZ Web Mercator (Google / OSM / Mapbox).
//!   `2^z × 2^z` tiles per zoom. Y=0 is north.
//! - [`web_mercator_tms`] — TMS-flipped Web Mercator. Same X math as
//!   `web_mercator`, but Y=0 is south (Y axis flipped).
//! - [`geodetic_tms`] — Global-geodetic TMS used by Cesium terrain
//!   (heightmap-1.0 / quantized-mesh-1.0). `2^(z+1) × 2^z` tiles per
//!   zoom; X covers 360° of longitude, Y covers 180° of latitude.
//!   Y=0 is south.
//!
//! All functions return `(x, y)` integer tile coordinates and accept
//! longitudes / latitudes in **degrees**. Out-of-range inputs are
//! clamped to the scheme's valid domain.

/// XYZ Web Mercator tiling: `2^z × 2^z` tiles per zoom, Y=0 north.
///
/// Used by OpenStreetMap, Google Maps, Mapbox raster tiles, etc.
pub mod web_mercator {
    /// Maximum latitude representable in Web Mercator (degrees).
    ///
    /// Derived from `2 * atan(e^π) - π/2 ≈ 85.05112878°`. Beyond this
    /// latitude the Mercator projection diverges.
    pub const MAX_LAT: f64 = 85.05112877980659;

    /// Number of tiles along each axis at `zoom`.
    #[inline]
    pub const fn tile_count(zoom: u8) -> u32 {
        1u32 << zoom
    }

    /// Convert `(lon, lat)` in degrees to XYZ tile coordinates at `zoom`.
    ///
    /// Longitudes are clamped to `[-180, 180]` and latitudes to
    /// `[-MAX_LAT, MAX_LAT]`.
    pub fn lonlat_to_tile(lon: f64, lat: f64, zoom: u8) -> (u32, u32) {
        let n = tile_count(zoom);
        let lon = lon.clamp(-180.0, 180.0);
        let lat = lat.clamp(-MAX_LAT, MAX_LAT);

        let x = (((lon + 180.0) / 360.0) * n as f64).floor() as u32;
        let lat_rad = lat.to_radians();
        let y =
            ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n as f64).floor() as u32;
        (x.min(n - 1), y.min(n - 1))
    }

    /// Convert XYZ tile `(z, x, y)` to its geographic bounds in degrees,
    /// returning `(west, south, east, north)`.
    pub fn tile_to_bounds(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
        let n = tile_count(z) as f64;
        let west = x as f64 / n * 360.0 - 180.0;
        let east = (x as f64 + 1.0) / n * 360.0 - 180.0;

        let lat_at = |yy: f64| -> f64 {
            let m = std::f64::consts::PI * (1.0 - 2.0 * yy / n);
            m.sinh().atan().to_degrees()
        };
        let north = lat_at(y as f64);
        let south = lat_at(y as f64 + 1.0);
        (west, south, east, north)
    }
}

/// TMS-flipped Web Mercator tiling. Same X math as [`web_mercator`], but
/// Y=0 is south (i.e. `tms_y = (2^z - 1) - xyz_y`).
pub mod web_mercator_tms {
    use super::web_mercator;

    /// Convert `(lon, lat)` to TMS-flipped Web Mercator tile coordinates.
    pub fn lonlat_to_tile(lon: f64, lat: f64, zoom: u8) -> (u32, u32) {
        let (x, xyz_y) = web_mercator::lonlat_to_tile(lon, lat, zoom);
        let max_y = web_mercator::tile_count(zoom) - 1;
        (x, max_y - xyz_y)
    }

    /// Convert TMS tile `(z, x, y)` to geographic bounds in degrees,
    /// returning `(west, south, east, north)`.
    pub fn tile_to_bounds(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
        let max_y = web_mercator::tile_count(z) - 1;
        web_mercator::tile_to_bounds(z, x, max_y - y)
    }

    /// Convert an XYZ Y coordinate to its TMS equivalent (or vice versa).
    #[inline]
    pub const fn flip_y(zoom: u8, y: u32) -> u32 {
        ((1u32 << zoom) - 1) - y
    }
}

/// Global-geodetic TMS tiling used by Cesium terrain (heightmap-1.0 /
/// quantized-mesh-1.0). `2^(z+1) × 2^z` tiles per zoom level. Y=0 is at
/// the south pole.
pub mod geodetic_tms {
    /// Number of tiles in the X direction at `zoom`.
    #[inline]
    pub const fn tile_count_x(zoom: u8) -> u32 {
        1u32 << (zoom + 1)
    }

    /// Number of tiles in the Y direction at `zoom`.
    #[inline]
    pub const fn tile_count_y(zoom: u8) -> u32 {
        1u32 << zoom
    }

    /// Convert `(lon, lat)` in degrees to geodetic TMS tile coordinates.
    pub fn lonlat_to_tile(lon: f64, lat: f64, zoom: u8) -> (u32, u32) {
        let lon = lon.clamp(-180.0, 180.0);
        let lat = lat.clamp(-90.0, 90.0);

        let nx = tile_count_x(zoom);
        let ny = tile_count_y(zoom);

        let x = (((lon + 180.0) / 360.0) * nx as f64).floor() as u32;
        let y = (((lat + 90.0) / 180.0) * ny as f64).floor() as u32;
        (x.min(nx - 1), y.min(ny - 1))
    }

    /// Convert geodetic TMS tile `(z, x, y)` to bounds in degrees,
    /// returning `(west, south, east, north)`.
    pub fn tile_to_bounds(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
        let nx = tile_count_x(z) as f64;
        let ny = tile_count_y(z) as f64;
        let west = x as f64 / nx * 360.0 - 180.0;
        let east = (x as f64 + 1.0) / nx * 360.0 - 180.0;
        let south = y as f64 / ny * 180.0 - 90.0;
        let north = (y as f64 + 1.0) / ny * 180.0 - 90.0;
        (west, south, east, north)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_mercator_origin_at_zoom_0_is_top_left() {
        // (lon=-180, lat=+MAX_LAT) sits at the NW corner of tile (0,0).
        let (x, y) = web_mercator::lonlat_to_tile(-180.0, web_mercator::MAX_LAT - 0.01, 0);
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn web_mercator_y_inverts_to_tms() {
        let zoom = 3;
        for y in 0..web_mercator::tile_count(zoom) {
            assert_eq!(web_mercator_tms::flip_y(zoom, y), (1 << zoom) - 1 - y);
            // Double-flip is identity.
            assert_eq!(
                web_mercator_tms::flip_y(zoom, web_mercator_tms::flip_y(zoom, y)),
                y
            );
        }
    }

    #[test]
    fn web_mercator_tile_to_bounds_roundtrip_for_centre_points() {
        let zoom = 4;
        for x in 0..web_mercator::tile_count(zoom) {
            for y in 0..web_mercator::tile_count(zoom) {
                let (w, s, e, n) = web_mercator::tile_to_bounds(zoom, x, y);
                let cx = (w + e) * 0.5;
                let cy = (s + n) * 0.5;
                let (x2, y2) = web_mercator::lonlat_to_tile(cx, cy, zoom);
                assert_eq!((x, y), (x2, y2), "tile centre should map back to its tile");
            }
        }
    }

    #[test]
    fn geodetic_tms_zoom_0_has_two_tiles_in_x() {
        assert_eq!(geodetic_tms::tile_count_x(0), 2);
        assert_eq!(geodetic_tms::tile_count_y(0), 1);
    }

    #[test]
    fn geodetic_tms_y0_is_south() {
        let (_, y) = geodetic_tms::lonlat_to_tile(0.0, -89.0, 4);
        assert_eq!(y, 0);
        let (_, y) = geodetic_tms::lonlat_to_tile(0.0, 89.0, 4);
        assert_eq!(y, geodetic_tms::tile_count_y(4) - 1);
    }

    #[test]
    fn geodetic_tms_bounds_cover_globe_at_zoom_0() {
        let (w0, s0, e0, n0) = geodetic_tms::tile_to_bounds(0, 0, 0);
        let (w1, _, e1, _) = geodetic_tms::tile_to_bounds(0, 1, 0);
        assert_eq!((w0, s0, e0, n0), (-180.0, -90.0, 0.0, 90.0));
        assert_eq!((w1, e1), (0.0, 180.0));
    }
}
