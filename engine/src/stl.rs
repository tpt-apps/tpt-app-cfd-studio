//! Pro edition: STL import and voxelisation for the 3D solver.
//!
//! [`parse_stl`] reads binary and ASCII STL into a [`Mesh`]; [`voxelize`]
//! scales the mesh so its extent along the *up* axis equals the requested
//! number of lattice cells, sizes a domain around it and rasterises it into a
//! solid mask by ray parity along the flow axis. Nothing here knows about the
//! solver: the result is a plain [`VoxelBody`] that [`crate::threed::Solver3D`]
//! consumes.
//!
//! Conventions (documented in the UI too): flow runs along the chosen axis of
//! the model, "up" is the chosen vertical axis, the third axis is lateral.
//! The default is flow along +X with +Z up, the usual CAD car orientation.

use crate::solve2d::CfdError;

/// Largest mesh accepted (triangles). A 3 M-triangle STL is ~150 MB binary;
/// beyond this the file is almost certainly not a single part.
const MAX_TRIANGLES: usize = 3_000_000;

/// Largest lattice the importer will build (cells). Keeps the desktop
/// wall-clock sane and memory bounded (each cell carries 19 f64 + fields).
pub const MAX_LATTICE_CELLS: usize = 2_200_000;

/// A triangle soup: three vertices per face, model units (unknown to us).
#[derive(Debug, Clone)]
pub struct Mesh {
    pub tris: Vec<[[f32; 3]; 3]>,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Mesh {
    fn from_tris(tris: Vec<[[f32; 3]; 3]>) -> Result<Mesh, CfdError> {
        if tris.len() < 4 {
            return Err(CfdError(format!(
                "STL has only {} triangles — need a closed solid (at least 4)",
                tris.len()
            )));
        }
        if tris.len() > MAX_TRIANGLES {
            return Err(CfdError(format!(
                "STL has {} triangles; the limit is {MAX_TRIANGLES}",
                tris.len()
            )));
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for t in &tris {
            for v in t {
                for a in 0..3 {
                    if !v[a].is_finite() {
                        return Err(CfdError("STL contains a non-finite coordinate".into()));
                    }
                    min[a] = min[a].min(v[a]);
                    max[a] = max[a].max(v[a]);
                }
            }
        }
        Ok(Mesh { tris, min, max })
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Model-space extent along each axis.
    pub fn extent(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

/// Parses a binary or ASCII STL file.
pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, CfdError> {
    // Binary STL: 80-byte header, u32 triangle count, 50 bytes per triangle.
    // Check the size relation first — many binary files begin with "solid".
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        if n > 0 && 84usize.checked_add(n.saturating_mul(50)) == Some(bytes.len()) {
            return parse_binary(bytes, n);
        }
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CfdError("not an STL file (neither binary nor ASCII)".into()))?;
    if !text.trim_start().to_ascii_lowercase().starts_with("solid") {
        return Err(CfdError("not an STL file (no `solid` header, wrong binary size)".into()));
    }
    parse_ascii(text)
}

fn parse_binary(bytes: &[u8], n: usize) -> Result<Mesh, CfdError> {
    let mut tris = Vec::with_capacity(n);
    let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    for i in 0..n {
        let base = 84 + i * 50 + 12; // skip the normal
        let mut t = [[0.0f32; 3]; 3];
        for (v, vert) in t.iter_mut().enumerate() {
            for (a, c) in vert.iter_mut().enumerate() {
                *c = f(base + v * 12 + a * 4);
            }
        }
        tris.push(t);
    }
    Mesh::from_tris(tris)
}

fn parse_ascii(text: &str) -> Result<Mesh, CfdError> {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next().map(|w| w.eq_ignore_ascii_case("vertex")) != Some(true) {
            continue;
        }
        let mut p = [0.0f32; 3];
        for c in p.iter_mut() {
            *c = it
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .ok_or_else(|| CfdError("malformed `vertex` line in ASCII STL".into()))?;
        }
        verts.push(p);
        if verts.len() > MAX_TRIANGLES * 3 {
            return Err(CfdError("ASCII STL is too large".into()));
        }
    }
    if verts.len() % 3 != 0 {
        return Err(CfdError("ASCII STL vertex count is not a multiple of 3".into()));
    }
    let tris = verts.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    Mesh::from_tris(tris)
}

/// Which model axis the flow runs along and which is vertical.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orientation {
    /// Model axis (0 = X, 1 = Y, 2 = Z) the free stream runs along.
    pub flow_axis: usize,
    /// +1.0: the stream runs toward +axis (nose at −axis); −1.0 reverses it.
    pub flow_sign: f32,
    /// Model axis that is vertical in the lattice.
    pub up_axis: usize,
    pub up_sign: f32,
}

impl Orientation {
    /// Flow along +X, +Z up (the usual CAD car orientation).
    pub const X_FLOW_Z_UP: Orientation = Orientation {
        flow_axis: 0,
        flow_sign: 1.0,
        up_axis: 2,
        up_sign: 1.0,
    };
    /// Flow along +X, +Y up.
    pub const X_FLOW_Y_UP: Orientation = Orientation {
        flow_axis: 0,
        flow_sign: 1.0,
        up_axis: 1,
        up_sign: 1.0,
    };

    pub fn reversed(mut self) -> Orientation {
        self.flow_sign = -self.flow_sign;
        self
    }

    fn lateral_axis(&self) -> usize {
        3 - self.flow_axis - self.up_axis
    }

    fn validate(&self) -> Result<(), CfdError> {
        if self.flow_axis > 2 || self.up_axis > 2 || self.flow_axis == self.up_axis {
            return Err(CfdError("flow and up must be two different axes".into()));
        }
        Ok(())
    }
}

/// A rasterised body plus the domain built around it.
#[derive(Debug, Clone)]
pub struct VoxelBody {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// `solid[(iz * ny + iy) * nx + ix]`
    pub solid: Vec<bool>,
    /// Body centre in lattice coordinates (for wake seeding).
    pub centre: (f64, f64, f64),
    /// Half the body length along the flow, in cells.
    pub half_chord: f64,
    /// Columns (along the flow) that crossed an odd number of surfaces and
    /// were left empty — a sign the mesh is not watertight.
    pub skipped_columns: usize,
    /// Columns the mesh's silhouette covers.
    pub covered_columns: usize,
}

/// Rasterise `mesh` into a lattice where its vertical extent is `h` cells.
pub fn voxelize(mesh: &Mesh, orient: Orientation, h: usize) -> Result<VoxelBody, CfdError> {
    orient.validate()?;
    if h < 4 {
        return Err(CfdError("need at least 4 cells across the body".into()));
    }
    let hf = h as f64;
    let (fa, ua, la) = (orient.flow_axis, orient.up_axis, orient.lateral_axis());
    let (fs, us) = (orient.flow_sign as f64, orient.up_sign as f64);

    // Model → (f, u, l) frame, then bounds.
    let to_flu = |v: &[f32; 3]| (fs * v[fa] as f64, us * v[ua] as f64, v[la] as f64);
    let (mut f0, mut f1) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut u0, mut u1) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut l0, mut l1) = (f64::INFINITY, f64::NEG_INFINITY);
    for t in &mesh.tris {
        for v in t {
            let (f, u, l) = to_flu(v);
            f0 = f0.min(f);
            f1 = f1.max(f);
            u0 = u0.min(u);
            u1 = u1.max(u);
            l0 = l0.min(l);
            l1 = l1.max(l);
        }
    }
    let ext_u = u1 - u0;
    if !(ext_u > 0.0) {
        return Err(CfdError(
            "the model has no extent along the chosen up axis — try the other up axis".into(),
        ));
    }
    let scale = hf / ext_u; // cells per model unit
    let len_cells = (f1 - f0) * scale;
    let wid_cells = (l1 - l0) * scale;

    // Domain: 1.8h upstream of the nose, 4.5h of wake behind the tail,
    // at least 4h tall and 4h wide (≈ 25 % blockage for a car-like body).
    let x_front = 1.8 * hf;
    let nx = (x_front + len_cells + 4.5 * hf).ceil() as usize;
    let ny = (4.0 * hf).max(hf + 3.0 * hf).ceil() as usize;
    let nz = (4.0 * hf).max(wid_cells + 2.6 * hf).ceil() as usize;
    let cells = nx.saturating_mul(ny).saturating_mul(nz);
    if cells > MAX_LATTICE_CELLS {
        return Err(CfdError(format!(
            "this body needs a {nx}×{ny}×{nz} lattice ({cells} cells); the limit is \
             {MAX_LATTICE_CELLS} — use a lower resolution, or a shorter/narrower body"
        )));
    }
    let cy = (ny - 1) as f64 / 2.0;
    let cz = (nz - 1) as f64 / 2.0;

    // Each triangle drops its x-intersections into the columns it covers.
    let mut hits: Vec<Vec<f32>> = vec![Vec::new(); ny * nz];
    // Sample just off the cell centre so rays never graze shared edges.
    const EY: f64 = 1.13e-3;
    const EZ: f64 = 1.71e-3;
    let place = |v: &[f32; 3]| {
        let (f, u, l) = to_flu(v);
        (
            x_front + (f - f0) * scale,
            cy + (u - 0.5 * (u0 + u1)) * scale,
            cz + (l - 0.5 * (l0 + l1)) * scale,
        )
    };
    for t in &mesh.tris {
        let (x0, y0, z0) = place(&t[0]);
        let (x1_, y1, z1) = place(&t[1]);
        let (x2, y2, z2) = place(&t[2]);
        let denom = (y1 - y0) * (z2 - z0) - (y2 - y0) * (z1 - z0);
        if denom.abs() < 1e-12 {
            continue; // edge-on to the flow, or degenerate
        }
        let (ylo, yhi) = (y0.min(y1).min(y2), y0.max(y1).max(y2));
        let (zlo, zhi) = (z0.min(z1).min(z2), z0.max(z1).max(z2));
        let iy_a = (ylo - EY).ceil().max(0.0) as usize;
        let iy_b = ((yhi - EY).floor().min((ny - 1) as f64)).max(-1.0);
        let iz_a = (zlo - EZ).ceil().max(0.0) as usize;
        let iz_b = ((zhi - EZ).floor().min((nz - 1) as f64)).max(-1.0);
        if iy_b < 0.0 || iz_b < 0.0 {
            continue;
        }
        for iz in iz_a..=(iz_b as usize) {
            let sz = iz as f64 + EZ;
            for iy in iy_a..=(iy_b as usize) {
                let sy = iy as f64 + EY;
                let w1 = ((sy - y0) * (z2 - z0) - (y2 - y0) * (sz - z0)) / denom;
                let w2 = ((y1 - y0) * (sz - z0) - (sy - y0) * (z1 - z0)) / denom;
                let w0 = 1.0 - w1 - w2;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    hits[iz * ny + iy].push((w0 * x0 + w1 * x1_ + w2 * x2) as f32);
                }
            }
        }
    }

    // Parity fill along x.
    let mut solid = vec![false; nx * ny * nz];
    let (mut skipped, mut covered) = (0usize, 0usize);
    for iz in 0..nz {
        for iy in 0..ny {
            let col = &mut hits[iz * ny + iy];
            if col.is_empty() {
                continue;
            }
            covered += 1;
            if col.len() % 2 != 0 {
                skipped += 1;
                continue;
            }
            col.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in col.chunks_exact(2) {
                let a = (pair[0] as f64).ceil().max(0.0) as usize;
                let b = ((pair[1] as f64).floor().min((nx - 1) as f64)).max(-1.0);
                if b < 0.0 {
                    continue;
                }
                for ix in a..=(b as usize) {
                    solid[(iz * ny + iy) * nx + ix] = true;
                }
            }
        }
    }
    if solid.iter().all(|&s| !s) {
        return Err(CfdError(
            "the mesh rasterised to nothing — it may not be a closed solid, or it is \
             thinner than one lattice cell (try a higher resolution or another up axis)"
                .into(),
        ));
    }

    Ok(VoxelBody {
        nx,
        ny,
        nz,
        solid,
        centre: (x_front + len_cells / 2.0, cy, cz),
        half_chord: len_cells / 2.0,
        skipped_columns: skipped,
        covered_columns: covered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned box as 12 triangles.
    fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
        let c = |x: usize, y: usize, z: usize| {
            [
                if x == 0 { lo[0] } else { hi[0] },
                if y == 0 { lo[1] } else { hi[1] },
                if z == 0 { lo[2] } else { hi[2] },
            ]
        };
        let quad = |a: [f32; 3], b: [f32; 3], cc: [f32; 3], d: [f32; 3]| {
            vec![[a, b, cc], [a, cc, d]]
        };
        let mut t = Vec::new();
        t.extend(quad(c(0, 0, 0), c(1, 0, 0), c(1, 1, 0), c(0, 1, 0)));
        t.extend(quad(c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)));
        t.extend(quad(c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)));
        t.extend(quad(c(0, 1, 0), c(1, 1, 0), c(1, 1, 1), c(0, 1, 1)));
        t.extend(quad(c(0, 0, 0), c(0, 1, 0), c(0, 1, 1), c(0, 0, 1)));
        t.extend(quad(c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)));
        t
    }

    fn to_binary(tris: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut b = vec![0u8; 80];
        b.extend((tris.len() as u32).to_le_bytes());
        for t in tris {
            b.extend([0u8; 12]);
            for v in t {
                for c in v {
                    b.extend(c.to_le_bytes());
                }
            }
            b.extend([0u8; 2]);
        }
        b
    }

    fn to_ascii(tris: &[[[f32; 3]; 3]]) -> String {
        let mut s = String::from("solid test\n");
        for t in tris {
            s.push_str("facet normal 0 0 0\n outer loop\n");
            for v in t {
                s.push_str(&format!("  vertex {} {} {}\n", v[0], v[1], v[2]));
            }
            s.push_str(" endloop\nendfacet\n");
        }
        s.push_str("endsolid test\n");
        s
    }

    /// UV sphere (closed, outward faces) of radius `r` at the origin.
    fn sphere_mesh(r: f32, rings: usize, segs: usize) -> Vec<[[f32; 3]; 3]> {
        let p = |i: usize, j: usize| {
            let th = std::f32::consts::PI * i as f32 / rings as f32;
            let ph = 2.0 * std::f32::consts::PI * j as f32 / segs as f32;
            [r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()]
        };
        let mut t = Vec::new();
        for i in 0..rings {
            for j in 0..segs {
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                if i != 0 {
                    t.push([a, b, d]);
                }
                if i != rings - 1 {
                    t.push([b, c, d]);
                }
            }
        }
        t
    }

    #[test]
    fn binary_and_ascii_round_trip_to_the_same_mesh() {
        let tris = box_mesh([0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        let bin = parse_stl(&to_binary(&tris)).unwrap();
        let asc = parse_stl(to_ascii(&tris).as_bytes()).unwrap();
        assert_eq!(bin.triangle_count(), 12);
        assert_eq!(asc.triangle_count(), 12);
        assert_eq!(bin.min, asc.min);
        assert_eq!(bin.max, asc.max);
        assert_eq!(bin.extent(), [2.0, 1.0, 1.0]);
    }

    #[test]
    fn binary_with_a_solid_header_is_still_binary() {
        let tris = box_mesh([0.0; 3], [1.0; 3]);
        let mut bytes = to_binary(&tris);
        bytes[..5].copy_from_slice(b"solid");
        assert_eq!(parse_stl(&bytes).unwrap().triangle_count(), 12);
    }

    #[test]
    fn garbage_and_tiny_files_are_rejected() {
        assert!(parse_stl(b"hello world").is_err());
        assert!(parse_stl(&[0u8; 10]).is_err());
        assert!(parse_stl(b"solid x\nendsolid x\n").is_err());
    }

    #[test]
    fn box_voxelises_to_its_volume() {
        // 2×1×1 box, Z up: vertical extent 1 → h cells, so 2h × h × h cells.
        let mesh = Mesh::from_tris(box_mesh([0.0; 3], [2.0, 1.0, 1.0])).unwrap();
        let h = 12;
        let v = voxelize(&mesh, Orientation::X_FLOW_Z_UP, h).unwrap();
        let n = v.solid.iter().filter(|&&s| s).count() as f64;
        let want = (2 * h * h * h) as f64;
        assert!((n - want).abs() / want < 0.12, "box voxels {n} vs {want}");
        assert_eq!(v.skipped_columns, 0);
        assert!(v.covered_columns > 0);
    }

    #[test]
    fn sphere_voxel_volume_matches_analytic() {
        let mesh = Mesh::from_tris(sphere_mesh(0.5, 48, 96)).unwrap();
        let h = 14;
        let v = voxelize(&mesh, Orientation::X_FLOW_Z_UP, h).unwrap();
        let n = v.solid.iter().filter(|&&s| s).count() as f64;
        let r = h as f64 / 2.0;
        let want = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
        assert!((n - want).abs() / want < 0.08, "sphere voxels {n} vs {want}");
        assert!(
            v.skipped_columns * 50 <= v.covered_columns,
            "closed sphere skipped {} of {} columns",
            v.skipped_columns,
            v.covered_columns
        );
    }

    #[test]
    fn orientation_picks_the_up_axis() {
        // A box 4× taller along Y than Z. With Y up that 4-unit extent is
        // h cells; with Z up the 1-unit extent is h cells, so everything is
        // 4× larger.
        let mesh = Mesh::from_tris(box_mesh([0.0; 3], [1.0, 4.0, 1.0])).unwrap();
        let y_up = voxelize(&mesh, Orientation::X_FLOW_Y_UP, 12).unwrap();
        let z_up = voxelize(&mesh, Orientation::X_FLOW_Z_UP, 12).unwrap();
        let count = |v: &VoxelBody| v.solid.iter().filter(|&&s| s).count();
        // Same slab, scaled by its vertical extent: 4× taller scale in Z-up.
        assert!(count(&z_up) > count(&y_up));
        assert!(Orientation::X_FLOW_Z_UP.reversed().flow_sign < 0.0);
    }

    #[test]
    fn open_mesh_is_flagged_not_silently_accepted() {
        // Drop the +X end cap (the last two triangles) so every ray through
        // the box sees a single surface.
        let mut tris = box_mesh([0.0; 3], [2.0, 1.0, 1.0]);
        tris.truncate(10);
        let mesh = Mesh::from_tris(tris).unwrap();
        let v = voxelize(&mesh, Orientation::X_FLOW_Z_UP, 12);
        match v {
            Ok(b) => assert!(b.skipped_columns > 0, "open mesh should skip columns"),
            Err(_) => {} // rasterised to nothing is also an honest outcome
        }
    }

    #[test]
    fn oversized_domains_are_refused() {
        let mesh = Mesh::from_tris(box_mesh([0.0; 3], [200.0, 1.0, 1.0])).unwrap();
        assert!(voxelize(&mesh, Orientation::X_FLOW_Z_UP, 16).is_err());
    }
}
