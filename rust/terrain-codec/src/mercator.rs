//! Reproject web-mercator (XYZ) DEM tiles onto a geodetic (EPSG:4326) grid.
//!
//! Elevation tiles are almost always served in the **web-mercator** XYZ
//! tiling (Terrarium, Mapbox Terrain-RGB, …), while Cesium quantized-mesh
//! terrain is served in the **geodetic TMS** scheme (`EPSG:4326`). Those are
//! different *projections*, not just different tilings — web-mercator's
//! latitude axis is non-linear — so producing a geodetic terrain tile means
//! **resampling** (warping) the mercator DEM, not merely cropping and
//! stitching it.
//!
//! [`MercatorDem`] holds a contiguous block of decoded web-mercator DEM
//! tiles stitched into one grid and lets you sample it by longitude /
//! latitude (bilinear). From there:
//!
//! - [`MercatorDem::geodetic_grid`] produces the `2^n+1` elevation grid that
//!   [`crate::terrain::encode_terrain`] expects, and
//! - [`MercatorDem::buffered_geodetic`] produces the halo-extended
//!   [`BufferedElevations`] that [`crate::terrain::NormalMode::BufferedGradient`]
//!   expects for seam-free normals.
//!
//! Fetching the source tiles is left to the caller (HTTP, disk, cache, …) so
//! this module stays free of any IO/async assumptions — supply an already
//! decoded tile via [`MercatorDem::new`] / [`MercatorDem::from_tiles`].
//!
//! # Example
//!
//! ```no_run
//! use terrain_codec::quantized_mesh::TileBounds;
//! use terrain_codec::mercator::MercatorDem;
//! use terrain_codec::terrain::{encode_terrain, TerrainOptions};
//! use terrain_codec::tile_coords::geodetic_tms;
//!
//! // Target geodetic TMS tile we want to emit.
//! let (w, s, e, n) = geodetic_tms::tile_to_bounds(12, 7252, 2852);
//! let bounds = TileBounds::new(w, s, e, n);
//! let grid_size = 257; // 2^8 + 1
//!
//! // Source web-mercator DEM: decide a source zoom, find the covering XYZ
//! // tiles, and assemble them (the closure does your fetch + heightmap decode).
//! let src_zoom = 13;
//! let tile_size = 512;
//! let (x0, y0, tx, ty) = MercatorDem::tiles_covering(src_zoom, w, s, e, n);
//! let dem = MercatorDem::from_tiles(src_zoom, x0, y0, tx, ty, tile_size, |z, x, y| {
//!     // fetch z/x/y.png, decode to elevations (tile_size², row-major N→S)
//!     # unimplemented!()
//! });
//!
//! let grid = dem.geodetic_grid(&bounds, grid_size);
//! let terrain = encode_terrain(&grid, grid_size, &bounds, &TerrainOptions::default());
//! ```

use std::f64::consts::PI;

use quantized_mesh::TileBounds;

use crate::normals::BufferedElevations;
use crate::tile_coords::web_mercator;

/// A contiguous rectangular block of decoded web-mercator (XYZ) DEM tiles,
/// stitched into a single elevation grid and sampleable by longitude /
/// latitude.
///
/// The stitched grid is row-major **north → south**, `tiles_x * tile_size`
/// columns by `tiles_y * tile_size` rows. Sampling outside the block clamps
/// to the nearest edge sample, so a halo that slightly overshoots the
/// fetched coverage degrades gracefully rather than panicking — but for
/// correct results the block should cover the target bounds (plus any halo).
#[derive(Debug, Clone)]
pub struct MercatorDem {
    zoom: u8,
    x0: u32,
    y0: u32,
    tiles_x: u32,
    tiles_y: u32,
    tile_size: u32,
    /// `(tiles_x*tile_size) × (tiles_y*tile_size)` elevations, row-major N→S.
    elev: Vec<f32>,
}

impl MercatorDem {
    /// Wrap an already-stitched elevation block.
    ///
    /// `elev` must be row-major north → south with
    /// `(tiles_x*tile_size) * (tiles_y*tile_size)` entries.
    ///
    /// # Panics
    ///
    /// Panics on a length mismatch, or if any tile dimension is zero.
    pub fn new(
        zoom: u8,
        x0: u32,
        y0: u32,
        tiles_x: u32,
        tiles_y: u32,
        tile_size: u32,
        elev: Vec<f32>,
    ) -> Self {
        assert!(
            tiles_x > 0 && tiles_y > 0 && tile_size > 0,
            "tiles_x, tiles_y and tile_size must be non-zero"
        );
        let expected = (tiles_x * tile_size) as usize * (tiles_y * tile_size) as usize;
        assert_eq!(
            elev.len(),
            expected,
            "stitched elevation length mismatch: expected {expected}, got {}",
            elev.len()
        );
        Self {
            zoom,
            x0,
            y0,
            tiles_x,
            tiles_y,
            tile_size,
            elev,
        }
    }

    /// Build a block by pulling each XYZ tile through `get_tile`, which
    /// returns that tile's decoded elevations (`tile_size²`, row-major
    /// north → south). The closure is where you do your fetch + heightmap
    /// decode; for async callers, pre-fetch into a map and look it up here.
    ///
    /// Tiles are requested in row-major order `(x0, y0) … (x0+tiles_x-1,
    /// y0+tiles_y-1)`.
    ///
    /// # Panics
    ///
    /// Panics if any returned tile does not have exactly `tile_size²` samples.
    pub fn from_tiles<F>(
        zoom: u8,
        x0: u32,
        y0: u32,
        tiles_x: u32,
        tiles_y: u32,
        tile_size: u32,
        mut get_tile: F,
    ) -> Self
    where
        F: FnMut(u8, u32, u32) -> Vec<f32>,
    {
        let ts = tile_size as usize;
        let w = (tiles_x * tile_size) as usize;
        let h = (tiles_y * tile_size) as usize;
        let mut elev = vec![0f32; w * h];

        for tj in 0..tiles_y {
            for ti in 0..tiles_x {
                let tile = get_tile(zoom, x0 + ti, y0 + tj);
                assert_eq!(
                    tile.len(),
                    ts * ts,
                    "tile {}/{}/{} has {} samples, expected {}",
                    zoom,
                    x0 + ti,
                    y0 + tj,
                    tile.len(),
                    ts * ts
                );
                let ox = ti as usize * ts;
                let oy = tj as usize * ts;
                for r in 0..ts {
                    let dst = (oy + r) * w + ox;
                    let src = r * ts;
                    elev[dst..dst + ts].copy_from_slice(&tile[src..src + ts]);
                }
            }
        }

        Self::new(zoom, x0, y0, tiles_x, tiles_y, tile_size, elev)
    }

    /// Range of XYZ tiles at `zoom` covering a longitude/latitude box,
    /// returning `(x0, y0, tiles_x, tiles_y)`.
    ///
    /// Widen the box by your halo before calling if you intend to sample a
    /// buffer beyond the tile (e.g. for [`buffered_geodetic`](Self::buffered_geodetic)).
    pub fn tiles_covering(
        zoom: u8,
        west: f64,
        south: f64,
        east: f64,
        north: f64,
    ) -> (u32, u32, u32, u32) {
        // North maps to the smaller Y, south to the larger Y.
        let (xw, yn) = web_mercator::lonlat_to_tile(west, north, zoom);
        let (xe, ys) = web_mercator::lonlat_to_tile(east, south, zoom);
        let x0 = xw.min(xe);
        let x1 = xw.max(xe);
        let y0 = yn.min(ys);
        let y1 = yn.max(ys);
        (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }

    /// Stitched-grid width in pixels (`tiles_x * tile_size`).
    #[inline]
    pub fn width_px(&self) -> u32 {
        self.tiles_x * self.tile_size
    }

    /// Stitched-grid height in pixels (`tiles_y * tile_size`).
    #[inline]
    pub fn height_px(&self) -> u32 {
        self.tiles_y * self.tile_size
    }

    /// Bilinearly sample the elevation at `(lon, lat)` in degrees.
    ///
    /// Latitude is clamped to the web-mercator limit. Positions outside the
    /// fetched block clamp to the nearest edge sample. `NaN` samples (e.g.
    /// missing data filled by the caller) are tolerated: the interpolation
    /// falls back to any defined neighbour, returning `NaN` only if all four
    /// corners are `NaN`.
    pub fn sample(&self, lon: f64, lat: f64) -> f32 {
        let n_tiles = 1u32 << self.zoom;
        let world_px = (n_tiles * self.tile_size) as f64;
        let lat = lat.clamp(-web_mercator::MAX_LAT, web_mercator::MAX_LAT);

        // Continuous global pixel coordinate, then local to the block, with a
        // half-pixel shift so integer indices land on pixel centres.
        let gx = (lon + 180.0) / 360.0 * world_px;
        let lat_rad = lat.to_radians();
        let gy = (1.0 - lat_rad.tan().asinh() / PI) / 2.0 * world_px;
        let lx = gx - (self.x0 * self.tile_size) as f64 - 0.5;
        let ly = gy - (self.y0 * self.tile_size) as f64 - 0.5;

        let w = self.width_px() as i64;
        let h = self.height_px() as i64;
        let fx = lx.floor();
        let fy = ly.floor();
        let tx = lx - fx;
        let ty = ly - fy;
        let clamp = |v: i64, max: i64| v.clamp(0, max - 1);
        let xi0 = clamp(fx as i64, w);
        let xi1 = clamp(fx as i64 + 1, w);
        let yi0 = clamp(fy as i64, h);
        let yi1 = clamp(fy as i64 + 1, h);
        let at = |xi: i64, yi: i64| -> f64 { self.elev[(yi * w + xi) as usize] as f64 };

        let top = bilerp(at(xi0, yi0), at(xi1, yi0), tx);
        let bot = bilerp(at(xi0, yi1), at(xi1, yi1), tx);
        bilerp(top, bot, ty) as f32
    }

    /// Build a [`RowSampler`] for a fixed latitude.
    ///
    /// This performs all the latitude-only work of [`sample`](Self::sample)
    /// — most importantly the web-mercator inverse `tan().asinh()` — once,
    /// so a whole grid row of longitudes then costs only linear index math
    /// plus a bilinear blend. The per-lon result is bit-for-bit identical to
    /// calling `sample(lon, lat)`.
    #[inline]
    fn row_sampler(&self, lat: f64) -> RowSampler<'_> {
        let n_tiles = 1u32 << self.zoom;
        let world_px = (n_tiles * self.tile_size) as f64;
        let lat = lat.clamp(-web_mercator::MAX_LAT, web_mercator::MAX_LAT);
        let lat_rad = lat.to_radians();
        let gy = (1.0 - lat_rad.tan().asinh() / PI) / 2.0 * world_px;

        let w = self.width_px() as i64;
        let h = self.height_px() as i64;
        let ly = gy - (self.y0 * self.tile_size) as f64 - 0.5;
        let fy = ly.floor();
        let ty = ly - fy;
        let fyi = fy as i64;
        let yi0 = fyi.clamp(0, h - 1);
        let yi1 = (fyi + 1).clamp(0, h - 1);

        RowSampler {
            elev: &self.elev,
            w,
            row0: yi0 * w,
            row1: yi1 * w,
            ty,
            world_px,
            x_base: (self.x0 * self.tile_size) as f64 + 0.5,
        }
    }

    /// Resample onto a geodetic `grid_size × grid_size` grid covering
    /// `bounds`, row-major north → south — ready for
    /// [`crate::terrain::encode_terrain`].
    ///
    /// # Panics
    ///
    /// Panics if `grid_size < 2`.
    pub fn geodetic_grid(&self, bounds: &TileBounds, grid_size: u32) -> Vec<f32> {
        assert!(grid_size >= 2, "grid_size must be >= 2");
        let gs = grid_size as usize;
        let lon_span = bounds.east - bounds.west;
        let lat_span = bounds.north - bounds.south;
        let denom = (grid_size - 1) as f64;
        let mut grid = vec![0f32; gs * gs];
        for j in 0..gs {
            // Row 0 = north. Latitude is fixed across the row, so the
            // web-mercator inverse (`tan().asinh()`) is done once here.
            let lat = bounds.north - (j as f64 / denom) * lat_span;
            let row = self.row_sampler(lat);
            let out = &mut grid[j * gs..j * gs + gs];
            for (i, cell) in out.iter_mut().enumerate() {
                let lon = bounds.west + (i as f64 / denom) * lon_span;
                *cell = row.sample_lon(lon);
            }
        }
        grid
    }

    /// Resample onto a halo-extended geodetic grid — a
    /// [`BufferedElevations`] for
    /// [`crate::terrain::NormalMode::BufferedGradient`].
    ///
    /// The inner `tile_grid_size × tile_grid_size` block matches
    /// [`geodetic_grid`](Self::geodetic_grid); the surrounding `buffer`-cell
    /// strip is sampled from the neighbour area (so make sure this
    /// `MercatorDem` was built to cover `bounds` widened by the halo).
    ///
    /// # Panics
    ///
    /// Panics if `tile_grid_size < 2`.
    pub fn buffered_geodetic(
        &self,
        bounds: &TileBounds,
        tile_grid_size: u32,
        buffer: u32,
    ) -> BufferedElevations {
        assert!(tile_grid_size >= 2, "tile_grid_size must be >= 2");
        let denom = (tile_grid_size - 1) as f64;
        let cell_lon = (bounds.east - bounds.west) / denom;
        let cell_lat = (bounds.north - bounds.south) / denom;
        let full = (tile_grid_size + 2 * buffer) as usize;
        let buf = buffer as f64;

        let mut elev = Vec::with_capacity(full * full);
        for j in 0..full {
            // j = buffer → north edge; rows increase southward. Latitude is
            // fixed across the row, so hoist the transcendental setup once.
            let lat = bounds.north + buf * cell_lat - (j as f64) * cell_lat;
            let row = self.row_sampler(lat);
            for i in 0..full {
                let lon = bounds.west - buf * cell_lon + (i as f64) * cell_lon;
                elev.push(row.sample_lon(lon) as f64);
            }
        }
        BufferedElevations::new(elev, tile_grid_size, buffer)
    }
}

/// Latitude-fixed sampling state for one output row (see
/// [`MercatorDem::row_sampler`]). Holds the two bracketing pixel rows and the
/// vertical blend factor precomputed, so [`sample_lon`](Self::sample_lon)
/// only has to resolve the longitude axis.
struct RowSampler<'a> {
    elev: &'a [f32],
    w: i64,
    /// `yi0 * w` — base offset of the north bracketing row.
    row0: i64,
    /// `yi1 * w` — base offset of the south bracketing row.
    row1: i64,
    ty: f64,
    world_px: f64,
    /// `x0 * tile_size + 0.5` — the half-pixel-shifted block origin.
    x_base: f64,
}

impl RowSampler<'_> {
    /// Sample the row at a single longitude. Equivalent to
    /// `MercatorDem::sample(lon, lat)` for this row's latitude.
    #[inline]
    fn sample_lon(&self, lon: f64) -> f32 {
        let lx = (lon + 180.0) / 360.0 * self.world_px - self.x_base;
        let fx = lx.floor();
        let tx = lx - fx;
        let fxi = fx as i64;
        let xi0 = fxi.clamp(0, self.w - 1);
        let xi1 = (fxi + 1).clamp(0, self.w - 1);
        let top = bilerp(
            self.elev[(self.row0 + xi0) as usize] as f64,
            self.elev[(self.row0 + xi1) as usize] as f64,
            tx,
        );
        let bot = bilerp(
            self.elev[(self.row1 + xi0) as usize] as f64,
            self.elev[(self.row1 + xi1) as usize] as f64,
            tx,
        );
        bilerp(top, bot, self.ty) as f32
    }
}

/// NaN-tolerant linear interpolation: falls back to a defined endpoint.
#[inline]
fn bilerp(a: f64, b: f64, t: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else {
        a * (1.0 - t) + b * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block sampled at the exact lon/lat of one of its posts should return
    /// that post's value (within float tolerance).
    #[test]
    fn sample_hits_pixel_centres() {
        let zoom = 4;
        let tile_size = 4;
        let (x0, y0) = (3, 5);
        let w = tile_size;
        let h = tile_size;
        // Distinct value per pixel so we can tell which one we hit.
        let elev: Vec<f32> = (0..(w * h)).map(|i| i as f32).collect();
        let dem = MercatorDem::new(zoom, x0, y0, 1, 1, tile_size, elev);

        // Reconstruct the lon/lat of pixel-centre (1, 2) in the block.
        let n_tiles = 1u32 << zoom;
        let world_px = (n_tiles * tile_size) as f64;
        let gx = (x0 * tile_size) as f64 + 1.0 + 0.5;
        let gy = (y0 * tile_size) as f64 + 2.0 + 0.5;
        let lon = gx / world_px * 360.0 - 180.0;
        // invert gy = (1 - asinh(tan lat)/PI)/2 * world_px
        let m = PI * (1.0 - 2.0 * gy / world_px);
        let lat = m.sinh().atan().to_degrees();

        let expected = (2 * w + 1) as f32; // row 2, col 1
        let got = dem.sample(lon, lat);
        assert!(
            (got - expected).abs() < 1e-3,
            "expected {expected}, got {got}"
        );
    }

    /// Bilinear sampling halfway between two posts averages them.
    #[test]
    fn sample_interpolates_between_posts() {
        let zoom = 4;
        let tile_size = 4;
        // Ramp in x: value == column index.
        let elev: Vec<f32> = (0..(tile_size * tile_size))
            .map(|i| (i % tile_size) as f32)
            .collect();
        let dem = MercatorDem::new(zoom, 0, 0, 1, 1, tile_size, elev);

        let world_px = ((1u32 << zoom) * tile_size) as f64;
        // Halfway between column 1 (centre gx=1.5) and column 2 (gx=2.5): gx=2.0.
        let lon = 2.0 / world_px * 360.0 - 180.0;
        let lat = 0.0; // any latitude inside the tile is fine for the x-ramp
        let got = dem.sample(lon, lat);
        assert!((got - 1.5).abs() < 1e-3, "expected ~1.5, got {got}");
    }

    #[test]
    fn tiles_covering_is_at_least_one_tile() {
        let (w, s, e, n) = web_mercator::tile_to_bounds(12, 3626, 1617);
        let (x0, y0, tx, ty) = MercatorDem::tiles_covering(12, w, s, e, n);
        // The source box is exactly one z12 tile, so it covers 1–2 tiles per axis.
        assert_eq!(x0, 3626);
        assert_eq!(y0, 1617);
        assert!((1..=2).contains(&tx));
        assert!((1..=2).contains(&ty));
    }

    #[test]
    fn geodetic_grid_of_flat_dem_is_flat() {
        let dem = MercatorDem::new(10, 0, 0, 1, 1, 8, vec![42.0f32; 64]);
        let bounds = TileBounds::new(0.0, 0.0, 1.0, 1.0);
        let grid = dem.geodetic_grid(&bounds, 17);
        assert_eq!(grid.len(), 17 * 17);
        assert!(grid.iter().all(|&v| (v - 42.0).abs() < 1e-4));
    }

    #[test]
    fn buffered_inner_block_matches_geodetic_grid() {
        // A smooth ramp so resampling is well-defined, then check the inner
        // block of the buffered grid equals the plain geodetic grid.
        let zoom = 10;
        let tile_size = 64;
        let elev: Vec<f32> = (0..(tile_size * tile_size))
            .map(|i| ((i % tile_size) + (i / tile_size)) as f32)
            .collect();
        let dem = MercatorDem::new(zoom, 100, 100, 1, 1, tile_size, elev);

        // Bounds well inside the tile so the halo stays in coverage.
        let (w, s, e, n) = web_mercator::tile_to_bounds(zoom, 100, 100);
        let inset_x = (e - w) * 0.2;
        let inset_y = (n - s) * 0.2;
        let bounds = TileBounds::new(w + inset_x, s + inset_y, e - inset_x, n - inset_y);

        let tile_grid = 33u32;
        let buffer = 2u32;
        let plain = dem.geodetic_grid(&bounds, tile_grid);
        let buffered = dem.buffered_geodetic(&bounds, tile_grid, buffer);

        let full = (tile_grid + 2 * buffer) as usize;
        let b = buffer as usize;
        let tg = tile_grid as usize;
        for j in 0..tg {
            for i in 0..tg {
                let inner = buffered.elevations[(j + b) * full + (i + b)] as f32;
                let p = plain[j * tg + i];
                assert!(
                    (inner - p).abs() < 1e-3,
                    "inner block mismatch at ({i},{j}): {inner} vs {p}"
                );
            }
        }
    }

    #[test]
    fn end_to_end_with_encode_terrain() {
        use crate::terrain::{TerrainOptions, encode_terrain};
        use quantized_mesh::DecodedMesh;

        let zoom = 12;
        let tile_size = 64;
        // A gentle bump so martini produces more than the corner triangles.
        let elev: Vec<f32> = (0..(tile_size * tile_size))
            .map(|i| {
                let x = (i % tile_size) as f32;
                let y = (i / tile_size) as f32;
                (x / 8.0).sin() * 20.0 + (y / 8.0).cos() * 15.0
            })
            .collect();
        let dem = MercatorDem::new(zoom, 3626, 1617, 1, 1, tile_size, elev);

        let (w, s, e, n) = web_mercator::tile_to_bounds(zoom, 3626, 1617);
        let bounds = TileBounds::new(w, s, e, n);
        let grid = dem.geodetic_grid(&bounds, 65);
        let bytes = encode_terrain(
            &grid,
            65,
            &bounds,
            &TerrainOptions {
                compression_level: 0,
                ..Default::default()
            },
        );
        let mesh = DecodedMesh::decode(&bytes).expect("decode");
        assert!(mesh.vertices.len() >= 4);
        assert!(mesh.indices.len() >= 6);
    }
}
