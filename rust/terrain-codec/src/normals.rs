//! Vertex-normal computation for terrain meshes.
//!
//! Two strategies are provided:
//!
//! - [`face_normals`] — accumulates triangle normals onto vertices.
//!   Simple but produces a **visible seam at tile boundaries** because
//!   each tile only sees its own triangles.
//! - [`buffered_gradient_normals`] — samples a buffer-extended DEM grid
//!   that covers cells **beyond** the tile on every side. Adjacent tiles
//!   read the same samples at any shared physical position, so seam
//!   vertices get identical normals on both sides and lighting is
//!   continuous.
//!
//! Both functions return ECEF normals (Cesium's convention for
//! oct-encoded normals stored in quantized-mesh tiles).

use quantized_mesh::coords::{WGS84_SEMI_MAJOR_AXIS, geodetic_to_ecef};
use quantized_mesh::{QUANTIZED_MAX, QuantizedVertices, TileBounds};

/// Elevations sampled on a grid that extends `buffer` cells *beyond* the
/// tile on each side.
///
/// Used by [`buffered_gradient_normals`] so that adjacent tiles read the
/// same DEM samples at any shared physical position. This is the same
/// idea as a raster "buffer" or image "padding": a strip of neighbour
/// data around the tile that lets edge vertices compute the same
/// gradient that the neighbour tile would compute.
///
/// # Layout
///
/// Row-major, north → south, `(tile_grid_size + 2*buffer)` samples per
/// side. The inner `tile_grid_size × tile_grid_size` block corresponds
/// to the tile itself; the surrounding strip of width `buffer` cells is
/// sampled from the **neighbour** tiles' DEM (or any DEM source covering
/// that physical area), so vertices on the absolute tile edge can still
/// read their `±1` neighbours via the buffer cells.
#[derive(Debug, Clone)]
pub struct BufferedElevations {
    /// Elevation samples (`(tile_grid_size + 2*buffer)²` entries,
    /// row-major north → south, NaN allowed for missing data).
    pub elevations: Vec<f64>,
    /// Number of buffer cells on each side.
    pub buffer: u32,
    /// Tile grid size (e.g. 65 for a Cesium-style 64-cell tile).
    /// The full buffered grid is `(tile_grid_size + 2*buffer)` per side.
    pub tile_grid_size: u32,
}

impl BufferedElevations {
    /// Create a buffer-extended elevation grid.
    ///
    /// # Panics
    ///
    /// Panics if `elevations.len()` does not equal
    /// `(tile_grid_size + 2*buffer)²`.
    pub fn new(elevations: Vec<f64>, tile_grid_size: u32, buffer: u32) -> Self {
        let side = (tile_grid_size + 2 * buffer) as usize;
        assert_eq!(
            elevations.len(),
            side * side,
            "buffered grid must have {side}×{side} = {} samples, got {}",
            side * side,
            elevations.len()
        );
        Self {
            elevations,
            buffer,
            tile_grid_size,
        }
    }

    /// Width/height of the full buffer-extended grid in samples.
    #[inline]
    pub fn buffered_grid_size(&self) -> u32 {
        self.tile_grid_size + 2 * self.buffer
    }
}

/// Compute vertex normals by accumulating triangle face normals onto each
/// vertex (then renormalising).
///
/// Vertices are mapped from their quantised `(u, v, h)` back to geodetic
/// `(lon, lat, height)` using the supplied bounds + height range, then to
/// ECEF for the cross-product math. The returned vectors are unit-length
/// ECEF normals — Cesium's convention for oct-encoded quantized-mesh
/// normals.
///
/// **Note:** this only looks at triangles inside the current tile, so
/// vertices on the tile edge get a normal that doesn't match the
/// neighbour tile's view of the same physical edge. Use
/// [`buffered_gradient_normals`] when seam-free shading matters.
pub fn face_normals(
    vertices: &QuantizedVertices,
    indices: &[u32],
    bounds: &TileBounds,
    min_height: f64,
    max_height: f64,
) -> Vec<[f32; 3]> {
    let vertex_count = vertices.len();
    let mut normals = vec![[0.0f32; 3]; vertex_count];

    // Convert quantized vertices to ECEF positions.
    let positions: Vec<[f64; 3]> = (0..vertex_count)
        .map(|i| {
            let u = vertices.u[i] as f64 / QUANTIZED_MAX as f64;
            let v = vertices.v[i] as f64 / QUANTIZED_MAX as f64;
            let h = vertices.height[i] as f64 / QUANTIZED_MAX as f64;

            let lon = bounds.west + u * (bounds.east - bounds.west);
            let lat = bounds.south + v * (bounds.north - bounds.south);
            let height = min_height + h * (max_height - min_height);

            geodetic_to_ecef(lon, lat, height)
        })
        .collect();

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }

        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;

        let p0 = &positions[i0];
        let p1 = &positions[i1];
        let p2 = &positions[i2];

        // Face normal via (p1-p0) × (p2-p0). With Martini's CCW winding
        // (when viewed from outside the ellipsoid) this gives an
        // outward-facing normal, which is Cesium's convention.
        let v1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let v2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];

        let normal = [
            (v1[1] * v2[2] - v1[2] * v2[1]) as f32,
            (v1[2] * v2[0] - v1[0] * v2[2]) as f32,
            (v1[0] * v2[1] - v1[1] * v2[0]) as f32,
        ];

        for &idx in &[i0, i1, i2] {
            normals[idx][0] += normal[0];
            normals[idx][1] += normal[1];
            normals[idx][2] += normal[2];
        }
    }

    for normal in &mut normals {
        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if len > 1e-6 {
            normal[0] /= len;
            normal[1] /= len;
            normal[2] /= len;
        } else {
            // Defensive fallback to local-up; should not happen with a
            // well-formed mesh.
            *normal = [0.0, 0.0, 1.0];
        }
    }

    normals
}

/// Compute vertex normals from the DEM gradient on a buffer-extended grid.
///
/// For every vertex the local east/north slope is read from the same DEM
/// samples that the neighbour tile would read — so the same physical edge
/// sees the same gradient from either side, eliminating the tile-boundary
/// shading seam that [`face_normals`] exhibits.
///
/// # Pipeline
///
/// 1. Map the vertex's `(u, v)` into the buffered grid's index space.
/// 2. Sample the buffered grid bilinearly at a half-cell stencil to get
///    the central differences `∂h/∂x` and `∂h/∂y` (metres per metre).
/// 3. Build a local ENU normal `(-∂h/∂x, -∂h/∂y, 1)` and rotate it into
///    ECEF, which is what Cesium expects for oct-encoded normals.
///
/// The returned vectors are unit-length ECEF normals.
pub fn buffered_gradient_normals(
    vertices: &QuantizedVertices,
    bounds: &TileBounds,
    buffered: &BufferedElevations,
) -> Vec<[f32; 3]> {
    let vertex_count = vertices.len();
    let buffer_cells = buffered.buffer as usize;
    let grid_size = buffered.tile_grid_size as usize;
    let full_grid_size = grid_size + 2 * buffer_cells;

    let tile_lon_span = bounds.east - bounds.west;
    let tile_lat_span = bounds.north - bounds.south;
    // Cells = (grid_size - 1) inside the tile; the buffer adds
    // `buffer_cells` cells on each side, so the per-cell step in degrees
    // is unchanged.
    let cell_lon_deg = tile_lon_span / (grid_size as f64 - 1.0);
    let cell_lat_deg = tile_lat_span / (grid_size as f64 - 1.0);

    // Metres per degree at the tile centre — good enough since a single
    // small tile is small relative to Earth's radius.
    let centre_lat = (bounds.south + bounds.north) * 0.5;
    let m_per_deg_lat = WGS84_SEMI_MAJOR_AXIS * std::f64::consts::PI / 180.0;
    let m_per_deg_lon = m_per_deg_lat * centre_lat.to_radians().cos();
    let cell_m_x = cell_lon_deg * m_per_deg_lon;
    let cell_m_y = cell_lat_deg * m_per_deg_lat;

    let height_at = |fx: f64, fy: f64| -> f64 {
        // Clamp to the buffered grid so vertices on the absolute tile edge
        // can still read their +/-1 neighbours via the buffer cells.
        let max_idx = (full_grid_size - 1) as f64;
        let fx = fx.clamp(0.0, max_idx);
        let fy = fy.clamp(0.0, max_idx);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(full_grid_size - 1);
        let y1 = (y0 + 1).min(full_grid_size - 1);
        let tx = fx - x0 as f64;
        let ty = fy - y0 as f64;
        let h00 = buffered.elevations[y0 * full_grid_size + x0];
        let h10 = buffered.elevations[y0 * full_grid_size + x1];
        let h01 = buffered.elevations[y1 * full_grid_size + x0];
        let h11 = buffered.elevations[y1 * full_grid_size + x1];
        // NaN-tolerant bilinear: fall back to any defined corner.
        let bilerp = |a: f64, b: f64, t: f64| -> f64 {
            if a.is_nan() {
                b
            } else if b.is_nan() {
                a
            } else {
                a * (1.0 - t) + b * t
            }
        };
        let top = bilerp(h00, h10, tx);
        let bot = bilerp(h01, h11, tx);
        let v = bilerp(top, bot, ty);
        if v.is_nan() { 0.0 } else { v }
    };

    let mut normals = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let u_norm = vertices.u[i] as f64 / QUANTIZED_MAX as f64;
        let v_norm = vertices.v[i] as f64 / QUANTIZED_MAX as f64;
        let lon = bounds.west + u_norm * tile_lon_span;
        let lat = bounds.south + v_norm * tile_lat_span;

        // Index space in the buffered grid:
        //   u_norm = 0 (tile west) ↔ buffered x index `buffer_cells`
        //   u_norm = 1 (tile east) ↔ buffered x index `buffer_cells + grid_size - 1`
        // Row index increases SOUTHWARD (north→south layout), so v_norm = 1
        // (tile north) maps to buffered y index `buffer_cells`.
        let fx_center = buffer_cells as f64 + u_norm * (grid_size as f64 - 1.0);
        let fy_center = buffer_cells as f64 + (1.0 - v_norm) * (grid_size as f64 - 1.0);

        // Central differences with a half-cell step. Bilinear interpolation
        // is exact at integer offsets, so the effective stencil width is
        // one cell — same as the neighbour tile's view of the same point.
        let h_west = height_at(fx_center - 0.5, fy_center);
        let h_east = height_at(fx_center + 0.5, fy_center);
        let h_north = height_at(fx_center, fy_center - 0.5);
        let h_south = height_at(fx_center, fy_center + 0.5);

        let dh_dx = (h_east - h_west) / cell_m_x; // east-positive
        let dh_dy = (h_north - h_south) / cell_m_y; // north-positive

        // Local ENU normal of z = h(x, y): gradient is (dh/dx, dh/dy), the
        // outward normal is (-grad, 1). Length doesn't matter — we
        // normalise after rotating to ECEF.
        let nx_enu = -dh_dx;
        let ny_enu = -dh_dy;
        let nz_enu = 1.0;

        // Rotate ENU → ECEF using the local east/north/up basis at (lat, lon).
        let lat_rad = lat.to_radians();
        let lon_rad = lon.to_radians();
        let (sin_lat, cos_lat) = lat_rad.sin_cos();
        let (sin_lon, cos_lon) = lon_rad.sin_cos();
        // east  = (-sin_lon,           cos_lon,            0)
        // north = (-sin_lat*cos_lon,  -sin_lat*sin_lon,    cos_lat)
        // up    = ( cos_lat*cos_lon,   cos_lat*sin_lon,    sin_lat)
        let ex = nx_enu * (-sin_lon) + ny_enu * (-sin_lat * cos_lon) + nz_enu * (cos_lat * cos_lon);
        let ey = nx_enu * cos_lon + ny_enu * (-sin_lat * sin_lon) + nz_enu * (cos_lat * sin_lon);
        let ez = ny_enu * cos_lat + nz_enu * sin_lat;

        let len = (ex * ex + ey * ey + ez * ez).sqrt();
        if len > 1e-12 {
            normals.push([(ex / len) as f32, (ey / len) as f32, (ez / len) as f32]);
        } else {
            // Degenerate — should never happen since the `up` component is
            // always ≥ cos(max slope), but be defensive.
            normals.push([
                (cos_lat * cos_lon) as f32,
                (cos_lat * sin_lon) as f32,
                sin_lat as f32,
            ]);
        }
    }

    normals
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A perfectly tilted plane h(x, y) = a*x + b*y has a constant ENU
    /// normal everywhere. The buffered-gradient path should reproduce that
    /// analytical normal (within float tolerance) for every vertex.
    #[test]
    fn buffered_gradient_matches_analytical_plane() {
        let grid_size = 65usize;
        let buffer = 1usize;
        let full_grid_size = grid_size + 2 * buffer;

        // ~1 km square near Tokyo so ENU→ECEF rotation does real work.
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let centre_lat = 35.005_f64;

        let m_per_deg_lat = WGS84_SEMI_MAJOR_AXIS * std::f64::consts::PI / 180.0;
        let m_per_deg_lon = m_per_deg_lat * centre_lat.to_radians().cos();
        let cell_lon_deg = (bounds.east - bounds.west) / (grid_size as f64 - 1.0);
        let cell_lat_deg = (bounds.north - bounds.south) / (grid_size as f64 - 1.0);
        let cell_m_x = cell_lon_deg * m_per_deg_lon;
        let cell_m_y = cell_lat_deg * m_per_deg_lat;

        // 5 m east-rise per cell, 3 m north-rise per cell.
        let dh_dx = 5.0 / cell_m_x;
        let dh_dy = 3.0 / cell_m_y;

        // Build a plane on the buffered grid. Row 0 is north → larger j means
        // smaller y in ENU, so reverse j when computing y.
        let mut buffered_grid = Vec::with_capacity(full_grid_size * full_grid_size);
        for j in 0..full_grid_size {
            for i in 0..full_grid_size {
                let x = (i as f64 - buffer as f64) * cell_m_x;
                let y = ((full_grid_size - 1 - j) as f64 - buffer as f64) * cell_m_y;
                buffered_grid.push(dh_dx * x + dh_dy * y + 100.0);
            }
        }

        // Hand-construct a few vertices spread across the tile.
        let mut vertices = QuantizedVertices::with_capacity(9);
        for v_q in [0u16, QUANTIZED_MAX / 2, QUANTIZED_MAX] {
            for u_q in [0u16, QUANTIZED_MAX / 2, QUANTIZED_MAX] {
                vertices.push(u_q, v_q, 0);
            }
        }

        let buffered_elev = BufferedElevations::new(buffered_grid, grid_size as u32, buffer as u32);
        let normals = buffered_gradient_normals(&vertices, &bounds, &buffered_elev);

        // Expected ENU normal of the plane (constant).
        let n_enu = {
            let v = [-dh_dx, -dh_dy, 1.0];
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / len, v[1] / len, v[2] / len]
        };

        for (i, n) in normals.iter().enumerate() {
            let u_norm = vertices.u[i] as f64 / QUANTIZED_MAX as f64;
            let v_norm = vertices.v[i] as f64 / QUANTIZED_MAX as f64;
            let lon = bounds.west + u_norm * (bounds.east - bounds.west);
            let lat = bounds.south + v_norm * (bounds.north - bounds.south);
            let lat_rad = lat.to_radians();
            let lon_rad = lon.to_radians();
            let (sin_lat, cos_lat) = lat_rad.sin_cos();
            let (sin_lon, cos_lon) = lon_rad.sin_cos();
            // Project the ECEF normal back to ENU and compare.
            let nx = n[0] as f64;
            let ny = n[1] as f64;
            let nz = n[2] as f64;
            let east_c = -sin_lon * nx + cos_lon * ny;
            let north_c = -sin_lat * cos_lon * nx + -sin_lat * sin_lon * ny + cos_lat * nz;
            let up_c = cos_lat * cos_lon * nx + cos_lat * sin_lon * ny + sin_lat * nz;
            let diff =
                (east_c - n_enu[0]).abs() + (north_c - n_enu[1]).abs() + (up_c - n_enu[2]).abs();
            assert!(
                diff < 1e-3,
                "vertex {i}: ENU normal mismatch (got [{east_c}, {north_c}, {up_c}], expected {n_enu:?})",
            );
        }
    }

    /// Two adjacent tiles sharing an east/west edge: with buffered-gradient
    /// normals, the seam vertices should get bit-identical normals because
    /// they read the same DEM samples from the shared buffer strip.
    #[test]
    fn buffered_gradient_normals_match_across_tile_seam() {
        let grid_size = 65usize;
        let buffer = 1usize;
        let full_grid_size = grid_size + 2 * buffer;

        let bounds_west = TileBounds::new(139.00, 35.0, 139.01, 35.01);
        let bounds_east = TileBounds::new(139.01, 35.0, 139.02, 35.01);

        // A simple ridge — both tiles' buffers read the same analytic field
        // at the seam, so seam normals must match.
        let height_field =
            |lon: f64, lat: f64| -> f64 { (lon * 100.0).sin() * 50.0 + (lat * 80.0).cos() * 30.0 };
        let cell_lon = (bounds_west.east - bounds_west.west) / (grid_size as f64 - 1.0);
        let cell_lat = (bounds_west.north - bounds_west.south) / (grid_size as f64 - 1.0);

        let make_buffered = |b: &TileBounds| -> Vec<f64> {
            let mut out = Vec::with_capacity(full_grid_size * full_grid_size);
            for j in 0..full_grid_size {
                let lat = b.north + cell_lat * buffer as f64 - (j as f64) * cell_lat;
                for i in 0..full_grid_size {
                    let lon = b.west - cell_lon * buffer as f64 + (i as f64) * cell_lon;
                    out.push(height_field(lon, lat));
                }
            }
            out
        };

        let buf_w =
            BufferedElevations::new(make_buffered(&bounds_west), grid_size as u32, buffer as u32);
        let buf_e =
            BufferedElevations::new(make_buffered(&bounds_east), grid_size as u32, buffer as u32);

        let mut v_west = QuantizedVertices::with_capacity(3);
        let mut v_east = QuantizedVertices::with_capacity(3);
        for v_q in [0u16, QUANTIZED_MAX / 2, QUANTIZED_MAX] {
            v_west.push(QUANTIZED_MAX, v_q, 0);
            v_east.push(0, v_q, 0);
        }

        let n_west = buffered_gradient_normals(&v_west, &bounds_west, &buf_w);
        let n_east = buffered_gradient_normals(&v_east, &bounds_east, &buf_e);

        for (i, (a, b)) in n_west.iter().zip(n_east.iter()).enumerate() {
            let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) as f64;
            assert!(
                dot > 0.999_999,
                "seam vertex {i}: normals diverged (dot = {dot})\nwest = {a:?}\neast = {b:?}",
            );
        }
    }

    #[test]
    fn face_normals_flat_returns_up() {
        // A flat patch at the equator/prime meridian: every face normal
        // should point ~radially out from Earth's centre.
        let bounds = TileBounds::new(-0.01, -0.01, 0.01, 0.01);
        let mut vertices = QuantizedVertices::with_capacity(4);
        vertices.push(0, 0, 0);
        vertices.push(QUANTIZED_MAX, 0, 0);
        vertices.push(0, QUANTIZED_MAX, 0);
        vertices.push(QUANTIZED_MAX, QUANTIZED_MAX, 0);
        let indices = vec![0u32, 1, 2, 1, 3, 2];

        let normals = face_normals(&vertices, &indices, &bounds, 0.0, 1.0);
        for n in &normals {
            // Near (0°, 0°) the outward radial direction is +X.
            assert!(n[0] > 0.99, "expected mostly-+X outward normal, got {n:?}");
        }
    }
}
