//! 3D turbulence (Smagorinsky LES) checks: stability at high Re and measured
//! drag coefficients against published references.
//!
//! These are slow (hundreds of thousands of lattice cells × thousands of
//! steps), so they are `#[ignore]`d by default:
//!
//! ```text
//! cargo test --release -p tpt-cfd-engine --features pro --test les3d \
//!     -- --ignored --nocapture
//! ```
//!
//! The measured values are printed, not tuned to pass: the bounds below are
//! deliberately wide because an LBM-LES run at 12 cells across the body is an
//! *estimate*, and the assertions only guard against a broken solver.

#![cfg(feature = "pro")]

use tpt_cfd_engine::threed::{Problem3D, Shape3D, Solver3D};
use tpt_cfd_engine::Fluid;

fn problem(shape: Shape3D, reynolds: f64, cells: usize) -> Problem3D {
    let fluid = Fluid::AIR;
    Problem3D {
        shape,
        body_height: 1.0,
        inlet_velocity: reynolds * fluid.kinematic_viscosity,
        fluid,
        cells_across_body: cells,
    }
}

fn run(shape: Shape3D, reynolds: f64, cells: usize, steps: usize) -> (f64, f64, bool) {
    let mut solver = Solver3D::new(&problem(shape, reynolds, cells)).expect("lays out");
    assert!(solver.info().les, "Re {reynolds} should engage the LES closure");
    solver.set_run_plan(steps);
    solver.run_to_completion();
    let cd = solver.drag_coefficient().expect("force averaged");
    let cl = solver.lift_coefficient().expect("force averaged");
    (cd, cl, solver.diverged())
}

#[test]
#[ignore = "slow: ~10⁵–10⁶ lattice cells"]
fn sphere_high_re_is_stable_and_near_reference() {
    // Published sub-critical sphere drag: Cd ≈ 0.4–0.5 over Re 10³–2·10⁵.
    let (cd, cl, diverged) = run(Shape3D::Sphere, 1.0e5, 12, 6_000);
    println!("sphere  Re=1e5  Cd={cd:.3}  Cl={cl:.3}  (reference ≈ 0.47)");
    assert!(!diverged, "LES sphere run went unstable");
    assert!((0.2..1.2).contains(&cd), "sphere Cd {cd:.3} is not a sane value");
}

#[test]
#[ignore = "slow: ~10⁵–10⁶ lattice cells"]
fn cube_high_re_is_stable_and_near_reference() {
    // Sharp-edged cube, face-on: Cd ≈ 1.05 (Re-independent once separated).
    let (cd, cl, diverged) = run(Shape3D::Cube, 1.0e5, 12, 6_000);
    println!("cube    Re=1e5  Cd={cd:.3}  Cl={cl:.3}  (reference ≈ 1.05)");
    assert!(!diverged, "LES cube run went unstable");
    assert!((0.5..2.0).contains(&cd), "cube Cd {cd:.3} is not a sane value");
}

#[test]
#[ignore = "slow: ~10⁵–10⁶ lattice cells"]
fn ahmed_body_high_re_is_stable_and_near_reference() {
    // Ahmed body, 25° slant: Cd ≈ 0.285 (Ahmed et al. 1984, Re ≈ 2.8·10⁶,
    // on a ground plane with 50 mm clearance, normalised by frontal area).
    // This model is in free stream with no ground plane and a rough lattice,
    // so only a loose sanity bound is asserted; the number is for the notes.
    let (cd, cl, diverged) = run(Shape3D::Ahmed, 1.0e6, 12, 8_000);
    println!("ahmed   Re=1e6  Cd={cd:.3}  Cl={cl:.3}  (reference ≈ 0.285, with ground)");
    assert!(!diverged, "LES Ahmed run went unstable");
    assert!((0.1..1.5).contains(&cd), "Ahmed Cd {cd:.3} is not a sane value");
}

/// A closed UV-sphere mesh (radius 0.5, centred on the origin).
fn uv_sphere(rings: usize, segs: usize) -> tpt_cfd_engine::stl::Mesh {
    let p = |i: usize, j: usize| {
        let th = std::f32::consts::PI * i as f32 / rings as f32;
        let ph = 2.0 * std::f32::consts::PI * j as f32 / segs as f32;
        [0.5 * th.sin() * ph.cos(), 0.5 * th.sin() * ph.sin(), 0.5 * th.cos()]
    };
    let mut bytes = vec![0u8; 80];
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    for i in 0..rings {
        for j in 0..segs {
            let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
            if i != 0 {
                tris.push([a, b, d]);
            }
            if i != rings - 1 {
                tris.push([b, c, d]);
            }
        }
    }
    bytes.extend((tris.len() as u32).to_le_bytes());
    for t in &tris {
        bytes.extend([0u8; 12]);
        for v in t {
            for c in v {
                bytes.extend(c.to_le_bytes());
            }
        }
        bytes.extend([0u8; 2]);
    }
    tpt_cfd_engine::stl::parse_stl(&bytes).expect("generated STL parses")
}

#[test]
#[ignore = "slow: ~10⁵–10⁶ lattice cells"]
fn imported_sphere_matches_the_preset_sphere() {
    // The importer must not change the physics: the same sphere, as a mesh
    // through STL → voxels → solver, should land close to the preset sphere.
    use tpt_cfd_engine::stl::{voxelize, Orientation};
    let (cd_preset, _, div_a) = run(Shape3D::Sphere, 1.0e5, 12, 6_000);

    let mesh = uv_sphere(48, 96);
    let body = voxelize(&mesh, Orientation::X_FLOW_Z_UP, 12).expect("voxelises");
    let mut p = problem(Shape3D::Imported, 1.0e5, 12);
    p.shape = Shape3D::Imported;
    let mut solver = Solver3D::from_voxels(&p, body).expect("lays out");
    solver.set_run_plan(6_000);
    solver.run_to_completion();
    let cd_imported = solver.drag_coefficient().expect("force averaged");
    println!("sphere  preset Cd={cd_preset:.3}  imported Cd={cd_imported:.3}");
    assert!(!div_a && !solver.diverged());
    assert!(
        (cd_imported - cd_preset).abs() / cd_preset < 0.25,
        "imported {cd_imported:.3} vs preset {cd_preset:.3}"
    );
}

/// Stability probe driven by environment variables (used while choosing the
/// LES defaults): PROBE_SHAPE=sphere|cube|ahmed, PROBE_RE, PROBE_STEPS.
#[test]
#[ignore = "manual probe"]
fn les_probe() {
    let shape = match std::env::var("PROBE_SHAPE").as_deref() {
        Ok("cube") => Shape3D::Cube,
        Ok("ahmed") => Shape3D::Ahmed,
        Ok("plate") => Shape3D::Plate,
        Ok("ellipsoid") => Shape3D::Ellipsoid { aspect: 4.0 },
        _ => Shape3D::Sphere,
    };
    let re: f64 = std::env::var("PROBE_RE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0e5);
    let steps: usize = std::env::var("PROBE_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(4_000);
    let cells: usize = std::env::var("PROBE_CELLS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let mut solver = Solver3D::new(&problem(shape, re, cells)).expect("lays out");
    solver.set_run_plan(steps);
    solver.run_to_completion();
    println!(
        "PROBE shape={:?} re={re} cells={cells} steps_run={}/{} diverged={} Cd={:?} tau={:.4} u={}",
        shape,
        solver.steps_run(),
        steps,
        solver.diverged(),
        solver.drag_coefficient(),
        solver.info().tau,
        solver.info().u_lattice
    );
}

#[test]
#[ignore = "slow: ~10⁵–10⁶ lattice cells"]
fn laminar_band_is_unchanged() {
    // Below the BGK ceiling the LES closure must stay off, so the validated
    // low-Re behaviour is exactly what it was.
    let solver = Solver3D::new(&problem(Shape3D::Sphere, 40.0, 12)).expect("lays out");
    assert!(!solver.info().les, "Re 40 must stay laminar");
}
