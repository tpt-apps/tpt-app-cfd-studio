//! 2D external-flow solve: [`Problem2D`] → an incrementally-steppable
//! [`Solver2D`] over tpt-phys-cfd's D2Q9 lattice, with drag/lift recovered
//! from boundary momentum exchange.
//!
//! Lattice units: δx = δt = ρ = 1. The physical problem maps onto the
//! lattice by matching Reynolds number: `ν_lattice = U_lattice·H/Re`, so
//! `τ = 3ν + ½` (D2Q9, `cs² = 1/3`). Because this is a BGK lattice with
//! `τ` bounded below by a stability floor (~0.52), there is a maximum
//! Reynolds number per (velocity, resolution) pair; pushing past it lowers
//! the simulated Re and flags [`LatticeInfo::reynolds_capped`] rather than
//! reporting a number the solver did not actually simulate.

use tpt_phys_cfd::{Lbm2D, XBoundary};

use crate::model::{Problem2D, Shape2D};

/// BGK stability floor: below this relaxation time the collision operator
/// goes unstable for bluff-body flows (sharp rasterised corners, vortex
/// shedding). 0.56 with the ladder top below caps the simulated Reynolds
/// ceiling at ≈ 5·H cells — the honest limit of a BGK D2Q9 solver
/// (measured: τ = 0.55 at Ma 0.22 diverged on a shedding cylinder); higher-Re
/// requests run here, capped and flagged (see [`LatticeInfo::reynolds_capped`]).
pub(crate) const TAU_MIN: f64 = 0.56;

/// Lattice-velocity ladder, low → high Mach (the top is ~Ma 0.17, where
/// compressibility error is still a few percent). Higher U means a higher
/// simulated ν for the same Re, i.e. τ further from the stability floor —
/// this is how moderate-to-high target Reynolds numbers stay stable.
pub(crate) const U_LADDER: [f64; 3] = [0.05, 0.075, 0.1];

/// A 2D solver error (bad problem parameters).
#[derive(Debug, Clone)]
pub struct CfdError(pub String);

impl std::fmt::Display for CfdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CfdError {}

/// How the physical problem was mapped onto the lattice, plus the physical
/// units of a lattice step (for the report and the UI's time readout).
#[derive(Debug, Clone)]
pub struct LatticeInfo {
    pub nx: usize,
    pub ny: usize,
    /// BGK relaxation time actually used.
    pub tau: f64,
    /// Lattice free-stream velocity actually used.
    pub u_lattice: f64,
    /// Lattice kinematic viscosity `cs²(τ−½)`.
    pub nu_lattice: f64,
    /// Reynolds number actually being simulated (may be below the target —
    /// see [`LatticeInfo::reynolds_capped`]).
    pub reynolds: f64,
    /// `true` when the requested Reynolds number exceeds this lattice's
    /// stability ceiling and the simulation ran at the ceiling instead.
    pub reynolds_capped: bool,
    /// Physical size of one lattice cell [m].
    pub cell_m: f64,
    /// Physical time of one lattice step [s].
    pub dt_s: f64,
    /// Rasterised frontal height of the body [cells] — the normalisation
    /// used for drag/lift coefficients.
    pub frontal_height_cells: usize,
}

/// An incremental 2D external-flow solver. Construct with [`Solver2D::new`],
/// advance with [`Solver2D::step_batch`] (a browser UI animates the flow
/// developing by stepping a few hundred lattice steps per frame), and read
/// drag/lift from the time-averaged force once the run has settled.
pub struct Solver2D {
    lbm: Lbm2D,
    problem: Problem2D,
    info: LatticeInfo,
    steps: usize,
    /// Step index from which force averaging starts (the wake needs the
    /// first half of the run to develop; averaging over the second half
    /// spans several shedding periods for unsteady cases).
    settle_at: usize,
    /// Total planned steps for [`Solver2D::run_to_completion`].
    planned_steps: usize,
    force_sum: [f64; 2],
    force_n: usize,
    /// Set when the fields blow up (BGK instability): stepping stops and the
    /// UI/report surface a warning instead of garbage.
    diverged: bool,
}

impl Solver2D {
    /// Lays out the lattice for `problem` and initialises a uniform stream.
    pub fn new(problem: &Problem2D) -> Result<Solver2D, CfdError> {
        if !(problem.inlet_velocity > 0.0) || !problem.inlet_velocity.is_finite() {
            return Err(CfdError("inlet velocity must be positive".into()));
        }
        if !(problem.body_height > 0.0) || !problem.body_height.is_finite() {
            return Err(CfdError("body height must be positive".into()));
        }
        if !(problem.fluid.kinematic_viscosity > 0.0) || !problem.fluid.kinematic_viscosity.is_finite()
        {
            return Err(CfdError("kinematic viscosity must be positive".into()));
        }
        let h = problem.cells_across_body;
        if !(8..=64).contains(&h) {
            return Err(CfdError(format!(
                "cells_across_body {h} outside the supported 8..=64 range"
            )));
        }
        // `h` must divide cleanly enough to keep the 20%-blockage / 12h
        // domain plan integer; the UI presets (12, 24) always do.

        // Domain: 20% blockage (ny = 5h) and 12h of streamwise length — the
        // body sits 3.5h from the inlet, leaving ~8h of wake to develop in.
        let ny = h * 5;
        let nx = h * 12;
        let cy = (ny - 1) as f64 / 2.0;
        let cx = (3.5 * h as f64).round();

        // Pick the lattice velocity/τ pair: the first (lowest-Mach) ladder
        // entry whose τ clears the stability floor for the target Re.
        let re_target = problem.reynolds();
        let mut u_lat = *U_LADDER.last().unwrap();
        let mut tau = TAU_MIN;
        let mut capped = true;
        for &u in &U_LADDER {
            let t = 3.0 * (u * h as f64 / re_target) + 0.5;
            if t >= TAU_MIN {
                u_lat = u;
                tau = t;
                capped = false;
                break;
            }
        }
        // Above the ladder top: run at the ceiling (floor τ) and report the
        // simulated Re honestly instead of pretending the target was met.
        let tau = tau_slip(tau);
        let nu_lat = (tau - 0.5) / 3.0; // ν = cs²(τ−½), cs² = 1/3

        let mut lbm = Lbm2D::new(nx, ny, tau);
        lbm.set_x_boundary(XBoundary::Inlet(u_lat));
        let half_chord = rasterize(&mut lbm, problem.shape, cx, cy, h as f64);

        // Frontal height actually rasterised (± a cell vs. `h` depending on
        // how the shape quantises); coefficients normalise by this.
        let frontal = (0..ny)
            .filter(|&iy| (0..nx).any(|ix| lbm.solid[iy * nx + ix]))
            .count();
        if frontal == 0 {
            return Err(CfdError("body rasterised to nothing at this resolution".into()));
        }

        lbm.initialise(1.0, [u_lat, 0.0]);
        // Seed the Kármán instability: a perfectly symmetric start can hold
        // the unstable steady wake branch for ~10⁵ steps before round-off
        // asymmetry grows (measured: Cd plateaus at ~0.91 vs the published
        // shedding mean 1.33 at Re = 100), which would silently time-average
        // the wrong flow. The bump goes just downstream of the body — in the
        // near-wake shear layers, where it actually trips them (a bump left
        // several heights behind a thin plate washes away without effect) —
        // slightly off the centreline, and decays harmlessly in genuinely
        // steady flows.
        lbm.add_velocity_bump(
            cx + half_chord + 0.35 * h as f64,
            cy + 0.12 * h as f64,
            0.6 * h as f64,
            [0.0, 0.3 * u_lat],
        );

        let planned_steps = DEFAULT_STEPS;
        let info = LatticeInfo {
            nx,
            ny,
            tau,
            u_lattice: u_lat,
            nu_lattice: nu_lat,
            reynolds: u_lat * frontal as f64 / nu_lat,
            reynolds_capped: capped,
            cell_m: problem.body_height / frontal as f64,
            dt_s: problem.body_height / frontal as f64 * u_lat / problem.inlet_velocity,
            frontal_height_cells: frontal,
        };

        Ok(Solver2D {
            lbm,
            problem: *problem,
            info,
            steps: 0,
            settle_at: planned_steps / 2,
            planned_steps,
            force_sum: [0.0; 2],
            force_n: 0,
            diverged: false,
        })
    }

    /// Lattice fields (velocity, density, solid mask) for rendering.
    pub fn lattice(&self) -> &Lbm2D {
        &self.lbm
    }

    pub fn info(&self) -> &LatticeInfo {
        &self.info
    }

    pub fn problem(&self) -> &Problem2D {
        &self.problem
    }

    /// Lattice steps taken so far.
    pub fn steps_run(&self) -> usize {
        self.steps
    }

    /// Total planned steps (see [`Solver2D::set_run_plan`]).
    pub fn planned_steps(&self) -> usize {
        self.planned_steps
    }

    /// `true` once [`Solver2D::run_to_completion`] has taken its planned
    /// step count (or the run diverged — check [`Solver2D::diverged`]).
    pub fn is_complete(&self) -> bool {
        self.diverged || self.steps >= self.planned_steps
    }

    /// `true` when the lattice went unstable (fields non-finite or
    /// velocities beyond any physical regime). Averages accumulated before
    /// the blow-up are still returned, but the UI/report must show a
    /// "solver diverged" warning rather than present them unqualified.
    pub fn diverged(&self) -> bool {
        self.diverged
    }

    /// Advance by `n` lattice steps, accumulating the boundary force into
    /// the post-settle average. Stops early if the run diverges.
    pub fn step_batch(&mut self, n: usize) {
        for _ in 0..n {
            if self.is_complete() {
                return;
            }
            self.step_exact(1);
        }
    }

    /// Advance by exactly `n` lattice steps regardless of the plan — how
    /// tests and the Pro report extend a finished run (e.g. to measure the
    /// shedding oscillation on a settled flow). Still stops on divergence.
    pub fn step_exact(&mut self, n: usize) {
        for _ in 0..n {
            if self.diverged {
                return;
            }
            self.lbm.step([0.0, 0.0]);
            self.steps += 1;
            if self.steps > self.settle_at {
                let f = self.lbm.boundary_force();
                self.force_sum[0] += f[0];
                self.force_sum[1] += f[1];
                self.force_n += 1;
            }
            // Cheap stability probe every 32 steps: a blown-up BGK run shows
            // up as absurd velocities or densities long before NaN.
            if self.steps % 32 == 0 && self.check_divergence() {
                self.diverged = true;
                return;
            }
        }
    }

    /// Run through the planned step count (no-op if already there).
    pub fn run_to_completion(&mut self) {
        while !self.is_complete() {
            self.step_batch(256);
        }
    }

    /// Current instantaneous force on the body, `[Fx, Fy]` (lattice units).
    pub fn instantaneous_force(&self) -> [f64; 2] {
        self.lbm.boundary_force()
    }

    /// Time-averaged drag coefficient since settling; `None` before the
    /// averaging window opens (the free edition's UI hides coefficients
    /// anyway — this is the Pro readout/report path).
    pub fn drag_coefficient(&self) -> Option<f64> {
        self.mean_coefficient(0)
    }

    /// Time-averaged lift coefficient since settling.
    pub fn lift_coefficient(&self) -> Option<f64> {
        self.mean_coefficient(1)
    }

    /// Override the planned run (total steps; averaging covers the second
    /// half). Resets the step counter and force averages, so call it right
    /// after construction — this is how tests and the Pro report ask for a
    /// longer, more highly averaged run than the UI's animated default.
    pub fn set_run_plan(&mut self, total_steps: usize) {
        assert!(total_steps >= 4, "run plan must be at least 4 steps");
        self.steps = 0;
        self.planned_steps = total_steps;
        self.settle_at = total_steps / 2;
        self.force_sum = [0.0; 2];
        self.force_n = 0;
        self.diverged = false;
    }

    fn mean_coefficient(&self, component: usize) -> Option<f64> {
        if self.force_n == 0 {
            return None;
        }
        let mean_force = self.force_sum[component] / self.force_n as f64;
        let q = 0.5 * self.info.u_lattice * self.info.u_lattice;
        Some(mean_force / (q * self.info.frontal_height_cells as f64))
    }

    /// Scans the fluid nodes' macroscopic fields for blow-up signatures.
    /// δx = δt = 1 and the free stream is ≤ 0.13, so anything past |u| ≈ 0.5
    /// or a density collapse is already unphysical — treat it as divergence.
    /// (Solid nodes are skipped: `macro_fields` zeroes their density.)
    fn check_divergence(&self) -> bool {
        self.lbm
            .solid
            .iter()
            .zip(
                self.lbm
                    .rho
                    .iter()
                    .zip(self.lbm.ux.iter().zip(self.lbm.uy.iter())),
            )
            .filter(|(&solid, _)| !solid)
            .any(|(_, (&rho, (&ux, &uy)))| {
                !rho.is_finite()
                    || rho < 0.2
                    || !ux.is_finite()
                    || ux.abs() > 0.5
                    || uy.abs() > 0.5
            })
    }
}

/// τ is already ≥ [`TAU_MIN`] when chosen; `tau_slip` only guards the
/// capping path where an explicit floor is applied.
pub(crate) fn tau_slip(tau: f64) -> f64 {
    tau.max(TAU_MIN)
}

/// Planned steps for the UI's animated default run: the seeded wake is
/// visibly developed (vortices shedding) well before this, and the run
/// completes in a few seconds of browser time at Quick resolution. The
/// averaging window covers the second half. Validation tests and the Pro
/// report extend this via [`Solver2D::set_run_plan`].
const DEFAULT_STEPS: usize = 7_500;

/// Rasterise `shape` onto the lattice's solid mask, centred at `(cx, cy)`,
/// with frontal height `h` cells. Returns the body's half-chord along the
/// flow (how far downstream the surface reaches) so the caller can seed the
/// near wake.
fn rasterize(lbm: &mut Lbm2D, shape: Shape2D, cx: f64, cy: f64, h: f64) -> f64 {
    match shape {
        Shape2D::Circle => {
            lbm.add_circle(cx, cy, h / 2.0);
            h / 2.0
        }
        Shape2D::Square => {
            let half = (h / 2.0).round() as isize;
            let (cx_i, cy_i) = (cx.round() as isize, cy.round() as isize);
            lbm.add_rect(
                (cx_i - half).max(0) as usize,
                (cy_i - half).max(0) as usize,
                (cx_i + half).max(0) as usize,
                (cy_i + half).max(0) as usize,
            );
            half as f64
        }
        Shape2D::FlatPlate => {
            // Plate normal to the flow: full height h, finite thickness
            // ≈ 0.18h (a 2D normal-plate benchmark geometry; a 1-2 cell
            // sliver under-resolves the separation edges and can hold an
            // unphysical steady bubble in the damped BGK lattice).
            let half_y = (h / 2.0).round() as isize;
            let half_x = ((h * 0.09).round() as isize).max(1);
            let (cx_i, cy_i) = (cx.round() as isize, cy.round() as isize);
            lbm.add_rect(
                (cx_i - half_x).max(0) as usize,
                (cy_i - half_y).max(0) as usize,
                (cx_i + half_x).max(0) as usize,
                (cy_i + half_y).max(0) as usize,
            );
            half_x as f64
        }
        Shape2D::Ellipse { aspect } => {
            let aspect = aspect.max(1.0);
            let a = h * aspect / 2.0; // semi-axis along the flow
            let b = h / 2.0; // semi-axis across it
            let nx = lbm.nx;
            let ny = lbm.ny;
            for iy in 0..ny {
                for ix in 0..nx {
                    let dx = (ix as f64 - cx) / a;
                    let dy = (iy as f64 - cy) / b;
                    if dx * dx + dy * dy <= 1.0 {
                        lbm.solid[iy * nx + ix] = true;
                    }
                }
            }
            a
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fluid;

    fn problem(shape: Shape2D, velocity: f64, h: usize) -> Problem2D {
        Problem2D {
            shape,
            body_height: 0.1,
            inlet_velocity: velocity,
            fluid: Fluid {
                density: 1.204,
                kinematic_viscosity: 1.516e-5,
            },
            cells_across_body: h,
        }
    }

    #[test]
    fn rejects_nonsensical_problems() {
        assert!(Solver2D::new(&problem(Shape2D::Circle, 0.0, 24)).is_err());
        assert!(Solver2D::new(&problem(Shape2D::Circle, -1.0, 24)).is_err());
        assert!(Solver2D::new(&problem(Shape2D::Circle, 3.0, 4)).is_err());
        assert!(Solver2D::new(&problem(Shape2D::Circle, 3.0, 128)).is_err());
    }

    #[test]
    fn lattice_layout_matches_blockage_and_length_plan() {
        // Re = U·H/ν = 3·0.1/1.516e-5 ≈ 19 800 is far past a BGK lattice —
        // use Re = 20 (U = 3 mm/s of air past a 10 cm cylinder) so the
        // mapping lands mid-ladder instead of at the stability ceiling.
        let velocity = 20.0 * 1.516e-5 / 0.1;
        let solver = Solver2D::new(&problem(Shape2D::Circle, velocity, 24)).unwrap();
        let info = solver.info();
        assert_eq!(info.ny, 120, "20% blockage");
        assert_eq!(info.nx, 288, "12h streamwise");
        assert!(
            (23..=25).contains(&info.frontal_height_cells),
            "disc rasterises ≈h rows, got {}",
            info.frontal_height_cells
        );
        assert!((info.reynolds - 20.0).abs() < 0.1, "re = {}", info.reynolds);
        assert!(!info.reynolds_capped);
        assert!((info.tau - 0.68).abs() < 1e-9, "τ = {}", info.tau);
    }

    #[test]
    fn coefficients_stay_none_before_settling() {
        let mut solver = Solver2D::new(&problem(Shape2D::Circle, 0.003, 16)).unwrap();
        solver.step_batch(200); // well short of settle_at
        assert!(solver.drag_coefficient().is_none());
        assert!(solver.lift_coefficient().is_none());
        // A short planned run completes instantly and opens the window.
        solver.set_run_plan(400);
        solver.run_to_completion();
        assert!(!solver.diverged());
        let cd = solver.drag_coefficient().expect("averaged after settling");
        assert!(cd.is_finite() && cd > 0.0, "cd = {cd}");
    }
}
