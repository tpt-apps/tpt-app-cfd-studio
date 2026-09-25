//! Published-reference validation cases (todo Phase 1): drag coefficient of
//! a circular cylinder in crossflow vs. Reynolds number, plus a normal flat
//! plate. These establish the accuracy bounds the product quotes: BGK D2Q9
//! at these resolutions reproduces cylinder Cd to roughly ±10–15%, so the
//! tests use deliberately loose bands — tight enough to catch a broken
//! solver, loose enough not to be brittle against the spread in the
//! published values themselves.
//!
//! Sized for `cargo test --release -p tpt-cfd-engine` (each case is a few
//! million to ~10⁹ lattice-cell updates; in debug these are 10–20× slower).
//!
//! References (experiments / established CFD benchmarks):
//! - Re = 20:  Cd ≈ 2.0–2.2   (Tritton 1959; Braza et al. 1986)
//! - Re = 40:  Cd ≈ 1.5–1.65  (Tritton; Norberg; eigenvalue benchmarks)
//! - Re = 100: Cd ≈ 1.33–1.40 (time-averaged; Braza et al.; Liu et al.),
//!   with periodic vortex shedding (Cl oscillating ~±0.3)
//! - Normal flat plate, Re ≈ 100: mean Cd ≈ 2.2–2.6 (2D blockage-sensitive;
//!   see e.g. LBM/NS benchmark compilations) — asserted loosely.

use tpt_cfd_engine::{Fluid, Problem2D, Shape2D, Solver2D};

/// Rescale the free-stream velocity so the problem hits `re` exactly
/// (Re = U·H/ν with H = 0.1 m and air ν below).
trait ScaledToRe {
    fn scaled_to_re(self, re: f64) -> Problem2D;
}

impl ScaledToRe for Problem2D {
    fn scaled_to_re(mut self, re: f64) -> Problem2D {
        self.inlet_velocity = re * self.fluid.kinematic_viscosity / self.body_height;
        self
    }
}

fn air() -> Fluid {
    Fluid {
        density: 1.204,
        kinematic_viscosity: 1.516e-5,
    }
}

fn cylinder(re: f64, cells: usize) -> Problem2D {
    Problem2D {
        shape: Shape2D::Circle,
        body_height: 0.1,
        inlet_velocity: 1.0,
        fluid: air(),
        cells_across_body: cells,
    }
    .scaled_to_re(re)
}

fn plate(re: f64) -> Problem2D {
    Problem2D {
        shape: Shape2D::FlatPlate,
        body_height: 0.1,
        inlet_velocity: 1.0,
        fluid: air(),
        cells_across_body: 24,
    }
    .scaled_to_re(re)
}

fn ellipse(re: f64, aspect: f64) -> Problem2D {
    Problem2D {
        shape: Shape2D::Ellipse { aspect },
        body_height: 0.1,
        inlet_velocity: 1.0,
        fluid: air(),
        cells_across_body: 24,
    }
    .scaled_to_re(re)
}

/// Runs one case to completion with a long averaging window.
fn settled(problem: &Problem2D, total_steps: usize) -> Solver2D {
    let mut solver = Solver2D::new(problem).expect("problem should lay out");
    solver.set_run_plan(total_steps);
    solver.run_to_completion();
    solver
}

#[test]
fn cylinder_re20_matches_published_drag() {
    let solver = settled(&cylinder(20.0, 24), 16_000);
    let cd = solver.drag_coefficient().expect("averaged after settling");
    // Measured 1.93 on this lattice (16k-step average).
    assert!(
        (1.65..=2.65).contains(&cd),
        "Cd(Re=20) = {cd:.3}, published ≈ 2.0–2.2"
    );
}

#[test]
fn cylinder_re40_matches_published_drag() {
    let solver = settled(&cylinder(40.0, 24), 20_000);
    let cd = solver.drag_coefficient().expect("averaged after settling");
    // Measured 1.39 on this lattice; published 1.5–1.65 (blockage and BGK
    // damping pull it slightly under the unbounded reference).
    assert!(
        (1.2..=1.8).contains(&cd),
        "Cd(Re=40) = {cd:.3}, published ≈ 1.5–1.65"
    );
}

#[test]
fn cylinder_re100_matches_published_drag_and_sheds() {
    // Unsteady regime: the mean comes from averaging over several shedding
    // periods (period ≈ H/(St·U) ≈ 2k lattice steps at these settings).
    let mut solver = Solver2D::new(&cylinder(100.0, 24)).expect("problem should lay out");
    solver.set_run_plan(60_000);
    solver.run_to_completion();
    let cd = solver.drag_coefficient().expect("averaged after settling");
    // Measured 1.37 on this lattice (published shedding mean ≈ 1.33–1.40;
    // the unseeded steady branch sits at ~0.91).
    assert!(
        (1.15..=1.6).contains(&cd),
        "mean Cd(Re=100) = {cd:.3}, published ≈ 1.33–1.40"
    );

    // Shedding: sample the instantaneous lift coefficient over one more
    // window — it must oscillate with a visible peak-to-peak amplitude.
    let mut cl_min = f64::INFINITY;
    let mut cl_max = f64::NEG_INFINITY;
    for _ in 0..4_000 {
        solver.step_exact(1);
        let (_, cl) = instant_drag_lift(&solver);
        cl_min = cl_min.min(cl);
        cl_max = cl_max.max(cl);
    }
    let amplitude = cl_max - cl_min;
    assert!(
        amplitude > 0.1,
        "no visible shedding: Cl swung only {amplitude:.4}"
    );
}

#[test]
fn normal_flat_plate_drag_bracket() {
    let solver = settled(&plate(100.0), 60_000);
    let cd = solver.drag_coefficient().expect("averaged after settling");
    // Benchmarks for a 2D normal plate at Re ≈ 100: mean Cd ≈ 2.2–2.6
    // (blockage-sensitive). The bracket guards the physical behaviour —
    // a normal plate must drag substantially more than a cylinder.
    assert!(
        (1.7..=3.2).contains(&cd),
        "plate Cd(Re=100) = {cd:.3}, benchmarks ≈ 2.2–2.6"
    );
}

#[test]
fn streamlined_body_drag_is_much_lower_than_bluff() {
    // The qualitative story the product tells: streamlining cuts drag. The
    // 4:1 ellipse must land clearly under the same-Re cylinder's
    // coefficient (measured ≈ 0.98 vs ≈ 1.37 on this lattice; the textbook
    // ~0.4–0.6 for 4:1 ellipses needs higher Re / less blockage).
    let cyl = settled(&cylinder(100.0, 24), 60_000);
    let ell = settled(&ellipse(100.0, 4.0), 60_000);
    let cd_cyl = cyl.drag_coefficient().unwrap();
    let cd_ell = ell.drag_coefficient().unwrap();
    assert!(
        cd_ell < 0.85 * cd_cyl,
        "4:1 ellipse Cd {cd_ell:.3} should be clearly under cylinder Cd {cd_cyl:.3}"
    );
    assert!(cd_ell > 0.0);
}

#[test]
fn capped_high_reynolds_stays_stable_and_honest() {
    // A full-size car at 30 m/s is Re ≈ 2·10⁵ — far past the BGK stability
    // ceiling. The solver must flag the cap, stay numerically stable, and
    // still produce a sane positive bluff-body drag (honestly labelled as
    // the capped-Re value, never presented as the target Re).
    let mut solver = Solver2D::new(&cylinder(200_000.0, 24)).expect("problem should lay out");
    assert!(solver.info().reynolds_capped);
    assert!(
        solver.info().reynolds < 500.0,
        "simulated Re {} must sit at the lattice ceiling",
        solver.info().reynolds
    );
    solver.run_to_completion();
    assert!(!solver.diverged(), "capped run went unstable");
    let cd = solver.drag_coefficient().unwrap();
    assert!(
        (0.5..3.0).contains(&cd),
        "capped-regime Cd = {cd:.3} is not a sane bluff-body value"
    );
}

/// Instantaneous force as a drag/lift coefficient pair (test helper).
fn instant_drag_lift(solver: &Solver2D) -> (f64, f64) {
    let info = solver.info();
    let [fx, fy] = solver.instantaneous_force();
    let q = 0.5 * info.u_lattice * info.u_lattice;
    (
        fx / (q * info.frontal_height_cells as f64),
        fy / (q * info.frontal_height_cells as f64),
    )
}
