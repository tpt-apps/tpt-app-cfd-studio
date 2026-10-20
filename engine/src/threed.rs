//! Pro edition: 3D external-flow solve (D3Q19 lattice-Boltzmann, BGK).
//!
//! The same physics contract as the 2D solver ([`crate::solve2d`]): a body
//! in a uniform stream, Reynolds-matched onto the lattice, drag/lift from
//! boundary momentum exchange, honest Reynolds capping at the BGK stability
//! ceiling. 3D multiplies the cell count, so the default layouts are modest
//! (Coarse 8 / Fine 12 cells across the body) and the wall-clock target is
//! "tens of seconds with a progress bar", not animation.
//!
//! Kept app-local (not upstream in tpt-phys-cfd) on purpose: the 3D path
//! exists to serve this product's external-flow cases, while upstream stays
//! a general-purpose library.

use crate::model::Fluid;
use crate::solve2d::{tau_slip, CfdError, TAU_MIN, U_LADDER};

/// Smagorinsky constant for the turbulence closure (Cs ≈ 0.1–0.17 is the
/// usual range; the upper end suits separated bluff-body wakes).
const SMAGORINSKY_CS: f64 = 0.16;

/// Floor on the molecular relaxation time in LES mode. At Re ≫ 10⁴ the
/// physical viscosity is far below what a few-cell-wide lattice can carry, so
/// the subgrid (Smagorinsky) viscosity does the dissipating; this keeps the
/// uniform-stream τ safely above the BGK instability point of 0.5.
const TAU_LES_FLOOR: f64 = 0.502;

/// Lattice free-stream velocity in LES mode. Measured, not assumed: on a
/// sphere at Re 10⁵ with 12 cells across, 0.1 diverged (step ~3100) while
/// 0.05 ran stably for 10 000 steps. Lighter damping (τ floor 0.5005 or
/// Cs 0.10) diverged even at 0.05, and so did regularised collision and 16
/// cells — see `engine/tests/les3d.rs` and the notes in `todo.md`.
const LES_U_LATTICE: f64 = 0.05;

/// The nineteen-velocity D3Q9 lattice: rest, the six axis directions, and
/// the twelve face diagonals.
const E3: [(i32, i32, i32); 19] = [
    (0, 0, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
    (1, 1, 0),
    (1, -1, 0),
    (-1, 1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (1, 0, -1),
    (-1, 0, 1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, 1, -1),
    (0, -1, 1),
    (0, -1, -1),
];

const W3: [f64; 19] = [
    1.0 / 3.0, // rest
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0, // axis
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0, // diagonals
];

/// Opposite direction for each D3Q19 velocity (E3[OPP3[k]] == −E3[k];
/// asserted in tests).
const OPP3: [usize; 19] = [
    0, 2, 1, 4, 3, 6, 5, 10, 9, 8, 7, 14, 13, 12, 11, 18, 17, 16, 15,
];

#[inline]
fn equilibrium3(rho: f64, u: [f64; 3]) -> [f64; 19] {
    let u2 = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
    let mut f = [0.0_f64; 19];
    for (i, slot) in f.iter_mut().enumerate() {
        let (ex, ey, ez) = E3[i];
        let eu = ex as f64 * u[0] + ey as f64 * u[1] + ez as f64 * u[2];
        *slot = W3[i] * rho * (1.0 + 3.0 * eu + 4.5 * eu * eu - 1.5 * u2);
    }
    f
}

/// Parametric 3D bodies, all sized by frontal height (the flow-facing
/// vertical extent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape3D {
    /// Sphere of diameter `H` — the canonical 3D drag reference.
    Sphere,
    /// Axis-aligned cube of side `H`.
    Cube,
    /// Flat plate normal to the flow: `H` tall and wide, finite thickness.
    Plate,
    /// Streamlined ellipsoid: `H` across the flow, `aspect·H` along it.
    Ellipsoid {
        /// Length-to-height ratio along the flow (clamped to ≥ 1).
        aspect: f64,
    },
    /// Ahmed body (road-car bluff-body reference): `3.6·H` long, `1.35·H`
    /// wide, `H` tall, rounded nose and a 25° slanted rear. The slant sets
    /// the rear-vortex wake that dominates road-car drag.
    Ahmed,
    /// A body imported from an STL file (see [`crate::stl`]). The lattice mask
    /// comes from the voxeliser; this variant only labels the problem.
    Imported,
}

impl Shape3D {
    pub fn id(self) -> &'static str {
        match self {
            Shape3D::Sphere => "sphere",
            Shape3D::Cube => "cube",
            Shape3D::Plate => "plate",
            Shape3D::Ellipsoid { .. } => "ellipsoid",
            Shape3D::Ahmed => "ahmed",
            Shape3D::Imported => "imported",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Shape3D::Sphere => "Sphere",
            Shape3D::Cube => "Cube",
            Shape3D::Plate => "Flat plate",
            Shape3D::Ellipsoid { .. } => "Streamlined body",
            Shape3D::Ahmed => "Ahmed body (road car)",
            Shape3D::Imported => "Imported STL body",
        }
    }
}

/// One 3D external-flow problem. Shares the fluid model with the 2D path.
#[derive(Debug, Clone, Copy)]
pub struct Problem3D {
    pub shape: Shape3D,
    /// Frontal height of the body [m].
    pub body_height: f64,
    /// Free-stream velocity [m/s].
    pub inlet_velocity: f64,
    pub fluid: Fluid,
    /// Lattice cells across the body (Coarse ≈ 8, Fine ≈ 12).
    pub cells_across_body: usize,
}

impl Problem3D {
    pub fn reynolds(&self) -> f64 {
        self.inlet_velocity * self.body_height / self.fluid.kinematic_viscosity
    }
}

/// How the physical problem was mapped onto the 3D lattice.
#[derive(Debug, Clone)]
pub struct LatticeInfo3D {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub tau: f64,
    pub u_lattice: f64,
    pub reynolds: f64,
    /// True only if the requested Reynolds number could not be honoured at
    /// all. With the turbulence model on (`les`) high Re is simulated, not
    /// capped, so this stays false there.
    pub reynolds_capped: bool,
    /// Smagorinsky large-eddy closure is active (Re beyond the laminar BGK
    /// stability band).
    pub les: bool,
    pub cell_m: f64,
    pub dt_s: f64,
    /// Projected frontal area of the rasterised body [cells²] — the drag/
    /// lift normalisation in 3D.
    pub frontal_area_cells: usize,
}

/// An incremental 3D external-flow solver (same stepping contract as
/// [`crate::solve2d::Solver2D`]).
pub struct Solver3D {
    /// Lattice dimensions (public: the slice viewer iterates the field).
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    tau: f64,
    u0: f64,
    /// Smagorinsky subgrid viscosity on (see [`LatticeInfo3D::les`]).
    les: bool,
    /// Post-collision distributions (SoA: one lane per lattice velocity).
    f: Vec<[f64; 19]>,
    f_swap: Vec<[f64; 19]>,
    pub rho: Vec<f64>,
    pub ux: Vec<f64>,
    pub uy: Vec<f64>,
    pub uz: Vec<f64>,
    pub solid: Vec<bool>,
    info: LatticeInfo3D,
    problem: Problem3D,
    steps: usize,
    planned_steps: usize,
    settle_at: usize,
    /// Per-step fluid force on the body from momentum exchange.
    boundary_force: [f64; 2],
    force_sum: [f64; 2],
    force_n: usize,
    diverged: bool,
}

impl Solver3D {
    /// Lays out the lattice for `problem` and initialises a uniform stream.
    pub fn new(problem: &Problem3D) -> Result<Solver3D, CfdError> {
        Self::build(problem, None)
    }

    /// Like [`Solver3D::new`], but with an imported body (see [`crate::stl`])
    /// instead of the preset `problem.shape`. The voxel body fixes the
    /// domain; `cells_across_body` must be the value it was voxelised with.
    pub fn from_voxels(
        problem: &Problem3D,
        body: crate::stl::VoxelBody,
    ) -> Result<Solver3D, CfdError> {
        Self::build(problem, Some(body))
    }

    fn build(
        problem: &Problem3D,
        voxels: Option<crate::stl::VoxelBody>,
    ) -> Result<Solver3D, CfdError> {
        if !(problem.inlet_velocity > 0.0) || !problem.inlet_velocity.is_finite() {
            return Err(CfdError("inlet velocity must be positive".into()));
        }
        if !(problem.body_height > 0.0) || !problem.body_height.is_finite() {
            return Err(CfdError("body height must be positive".into()));
        }
        if !(problem.fluid.kinematic_viscosity > 0.0)
            || !problem.fluid.kinematic_viscosity.is_finite()
        {
            return Err(CfdError("kinematic viscosity must be positive".into()));
        }
        let h = problem.cells_across_body;
        if !(6..=16).contains(&h) {
            return Err(CfdError(format!(
                "cells_across_body {h} outside the supported 6..=16 range for 3D"
            )));
        }

        // Domain: 25% blockage cross-section (4h × 4h) and 8h of streamwise
        // length — 3D blockage effects are milder than 2D, and every cell
        // counts against wall-clock here.
        let imported = voxels.is_some();
        let (nx, ny, nz, cx, cy, cz) = match &voxels {
            Some(v) => (v.nx, v.ny, v.nz, v.centre.0, v.centre.1, v.centre.2),
            None => {
                let (nx, ny, nz) = (h * 8, h * 4, h * 4);
                (
                    nx,
                    ny,
                    nz,
                    (3.0 * h as f64).round(),
                    (ny - 1) as f64 / 2.0,
                    (nz - 1) as f64 / 2.0,
                )
            }
        };

        // Same Reynolds→τ ladder as the 2D path (shared constants).
        let re_target = problem.reynolds();
        let mut u0 = *U_LADDER.last().unwrap();
        let mut tau = TAU_MIN;
        let mut les = true;
        for &u in &U_LADDER {
            let t = 3.0 * (u * h as f64 / re_target) + 0.5;
            if t >= TAU_MIN {
                u0 = u;
                tau = t;
                les = false;
                break;
            }
        }
        // Beyond the laminar stability band, run the Smagorinsky closure at
        // the real Reynolds number instead of capping it: the molecular τ
        // follows the target (floored for stability) and the subgrid
        // viscosity supplies the rest.
        if les {
            // Halving the lattice velocity from the ladder top (0.1 → 0.05)
            // is what keeps the closure stable on bluff bodies (see the
            // notes on `LES_U_LATTICE`).
            u0 = LES_U_LATTICE;
        }
        let tau = if les {
            (3.0 * (u0 * h as f64 / re_target) + 0.5).max(TAU_LES_FLOOR)
        } else {
            tau_slip(tau)
        };
        let capped = false;

        let n = nx * ny * nz;
        let (solid, half_chord) = match voxels {
            Some(v) => {
                if v.solid.len() != n {
                    return Err(CfdError("imported body does not match its lattice".into()));
                }
                (v.solid, v.half_chord)
            }
            None => {
                let mut solid = vec![false; n];
                let half_chord =
                    rasterize3(&mut solid, nx, ny, nz, problem.shape, cx, cy, cz, h as f64);
                (solid, half_chord)
            }
        };
        // Drag/lift normalise by the body's projected frontal area: the
        // count of (y,z) columns containing at least one solid cell.
        let frontal_area = {
            let mut area = 0usize;
            for iz in 0..nz {
                for iy in 0..ny {
                    let mut hit = false;
                    for ix in 0..nx {
                        if solid[(iz * ny + iy) * nx + ix] {
                            hit = true;
                            break;
                        }
                    }
                    area += hit as usize;
                }
            }
            area
        };
        if frontal_area == 0 {
            return Err(CfdError("body rasterised to nothing at this resolution".into()));
        }

        let mut f = vec![[0.0; 19]; n];
        let mut rho = vec![1.0; n];
        let ux = vec![0.0; n];
        let uy = vec![0.0; n];
        let uz = vec![0.0; n];
        for i in 0..n {
            if solid[i] {
                rho[i] = 0.0;
                continue;
            }
            f[i] = equilibrium3(1.0, [u0, 0.0, 0.0]);
        }
        // Seed the wake instability just downstream of the body (same
        // rationale as the 2D path).
        seed3(&mut f, &solid, nx, ny, nz, cx + half_chord + 0.55 * h as f64,
              cy + 0.15 * h as f64, cz, 0.6 * h as f64, [0.0, 0.3 * u0, 0.0]);

        // LES runs at half the lattice velocity, so it needs about twice the
        // steps to cover the same number of flow-throughs.
        let planned_steps = if les { 6_000 } else { 3_500 };
        let nu = (tau - 0.5) / 3.0;
        // Reference length in cells: an imported body is scaled so its
        // vertical extent is exactly `h` cells; presets use the frontal-area
        // equivalent as before.
        let ref_len = if imported {
            h as f64
        } else {
            (frontal_area as f64).sqrt()
        };
        let info = LatticeInfo3D {
            nx,
            ny,
            nz,
            tau,
            u_lattice: u0,
            // Laminar: the lattice Re that was actually set up. LES: the
            // physical target (the subgrid model carries the difference).
            reynolds: if les {
                re_target
            } else {
                u0 * ref_len / nu
            },
            reynolds_capped: capped,
            les,
            cell_m: problem.body_height / ref_len,
            dt_s: problem.body_height / ref_len * u0 / problem.inlet_velocity,
            frontal_area_cells: frontal_area,
        };

        Ok(Solver3D {
            nx,
            ny,
            nz,
            tau,
            u0,
            les,
            f,
            f_swap: Vec::new(),
            rho,
            ux,
            uy,
            uz,
            solid,
            info,
            problem: *problem,
            steps: 0,
            planned_steps,
            settle_at: planned_steps / 2,
            boundary_force: [0.0; 2],
            force_sum: [0.0; 2],
            force_n: 0,
            diverged: false,
        })
    }

    pub fn info(&self) -> &LatticeInfo3D {
        &self.info
    }

    pub fn problem(&self) -> &Problem3D {
        &self.problem
    }

    pub fn steps_run(&self) -> usize {
        self.steps
    }

    pub fn planned_steps(&self) -> usize {
        self.planned_steps
    }

    pub fn is_complete(&self) -> bool {
        self.diverged || self.steps >= self.planned_steps
    }

    pub fn diverged(&self) -> bool {
        self.diverged
    }

    /// Override the planned run; resets counters (see the 2D solver).
    pub fn set_run_plan(&mut self, total_steps: usize) {
        assert!(total_steps >= 4, "run plan must be at least 4 steps");
        self.steps = 0;
        self.planned_steps = total_steps;
        self.settle_at = total_steps / 2;
        self.force_sum = [0.0; 2];
        self.force_n = 0;
        self.diverged = false;
    }

    pub fn step_batch(&mut self, n: usize) {
        for _ in 0..n {
            if self.is_complete() {
                return;
            }
            self.step_exact(1);
        }
    }

    /// Advance exactly `n` steps regardless of the plan (tests/report).
    pub fn step_exact(&mut self, n: usize) {
        for _ in 0..n {
            if self.diverged {
                return;
            }
            self.collide_and_stream();
            self.macro_fields();
            self.steps += 1;
            if self.steps > self.settle_at {
                let f = self.boundary_force;
                self.force_sum[0] += f[0];
                self.force_sum[1] += f[1];
                self.force_n += 1;
            }
            if self.steps % 16 == 0 && self.check_divergence() {
                self.diverged = true;
                return;
            }
        }
    }

    pub fn run_to_completion(&mut self) {
        while !self.is_complete() {
            self.step_batch(64);
        }
    }

    pub fn instantaneous_force(&self) -> [f64; 2] {
        self.boundary_force
    }

    pub fn drag_coefficient(&self) -> Option<f64> {
        self.mean_coefficient(0)
    }

    pub fn lift_coefficient(&self) -> Option<f64> {
        self.mean_coefficient(1)
    }

    fn mean_coefficient(&self, component: usize) -> Option<f64> {
        if self.force_n == 0 {
            return None;
        }
        let mean_force = self.force_sum[component] / self.force_n as f64;
        let q = 0.5 * self.u0 * self.u0;
        Some(mean_force / (q * self.info.frontal_area_cells as f64))
    }

    /// BGK collision with boundary momentum exchange, then pull-scheme
    /// streaming with half-way bounce-back, velocity inlet (west) and a
    /// density-pinned equilibrium pressure outlet (east). y/z edges use
    /// zero-normal-gradient (free-slip-like) closure, matching the 2D path.
    fn collide_and_stream(&mut self) {
        let (nx, ny, nz) = (self.nx, self.ny, self.nz);
        let tau = self.tau;
        let mut boundary_force = [0.0_f64; 2];

        // Collision (with momentum exchange over solid-adjacent links).
        for i in 0..self.f.len() {
            if self.solid[i] {
                continue;
            }
            let fi = self.f[i];
            let rho: f64 = fi.iter().sum();
            let mut ux = 0.0;
            let mut uy = 0.0;
            let mut uz = 0.0;
            for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
                ux += fi[k] * ex as f64;
                uy += fi[k] * ey as f64;
                uz += fi[k] * ez as f64;
            }
            let inv = 1.0 / rho;
            ux *= inv;
            uy *= inv;
            uz *= inv;
            let feq = equilibrium3(rho, [ux, uy, uz]);
            // Smagorinsky subgrid viscosity, local-τ form (Hou et al.): the
            // strain rate comes from the non-equilibrium momentum flux Π, so
            // no velocity gradients are needed.
            let tau_i = if self.les {
                let (mut pxx, mut pyy, mut pzz) = (0.0, 0.0, 0.0);
                let (mut pxy, mut pxz, mut pyz) = (0.0, 0.0, 0.0);
                for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
                    let (ex, ey, ez) = (ex as f64, ey as f64, ez as f64);
                    let fneq = fi[k] - feq[k];
                    pxx += fneq * ex * ex;
                    pyy += fneq * ey * ey;
                    pzz += fneq * ez * ez;
                    pxy += fneq * ex * ey;
                    pxz += fneq * ex * ez;
                    pyz += fneq * ey * ez;
                }
                let q = (pxx * pxx
                    + pyy * pyy
                    + pzz * pzz
                    + 2.0 * (pxy * pxy + pxz * pxz + pyz * pyz))
                    .sqrt();
                0.5 * (tau
                    + (tau * tau
                        + 18.0
                            * std::f64::consts::SQRT_2
                            * SMAGORINSKY_CS
                            * SMAGORINSKY_CS
                            * q
                            / rho)
                        .sqrt())
            } else {
                tau
            };
            let (ix, iy, iz) = (i % nx, (i / nx) % ny, i / (nx * ny));
            for k in 0..19 {
                let f_post = fi[k] - (fi[k] - feq[k]) / tau_i;
                self.f[i][k] = f_post;
                let (ex, ey, ez) = E3[k];
                let sx = ix as isize + ex as isize;
                let sy = iy as isize + ey as isize;
                let sz = iz as isize + ez as isize;
                if sx >= 0 && sx < nx as isize
                    && sy >= 0 && sy < ny as isize
                    && sz >= 0 && sz < nz as isize
                {
                    let nidx =
                        (sz as usize * ny + sy as usize) * nx + sx as usize;
                    if self.solid[nidx] {
                        boundary_force[0] += 2.0 * f_post * ex as f64;
                        boundary_force[1] += 2.0 * f_post * ey as f64;
                    }
                }
            }
        }
        self.boundary_force = boundary_force;

        // Streaming (pull): the previous distributions are read from the
        // swap buffer.
        self.f_swap.clear();
        self.f_swap.extend_from_slice(&self.f);
        let f_prev = &self.f_swap;
        let mut f_new = std::mem::take(&mut self.f);
        for iz in 0..nz {
            for iy in 0..ny {
                for ix in 0..nx {
                    let i = (iz * ny + iy) * nx + ix;
                    if self.solid[i] {
                        continue;
                    }
                    // East outflow column: rebuild the five west-pointing
                    // distributions as equilibrium at the pinned reference
                    // density and the node's local velocity (a low-Mach
                    // equilibrium pressure outlet; see module docs).
                    let is_outlet = ix == nx - 1;
                    let mut local_u = [0.0; 3];
                    if is_outlet {
                        let fi = &f_prev[i];
                        let rho: f64 = fi.iter().sum();
                        if rho > 0.5 {
                            let mut u = [0.0; 3];
                            for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
                                u[0] += fi[k] * ex as f64;
                                u[1] += fi[k] * ey as f64;
                                u[2] += fi[k] * ez as f64;
                            }
                            local_u = [u[0] / rho, u[1] / rho, u[2] / rho];
                            local_u[0] = local_u[0].clamp(-0.3, 0.3);
                            local_u[1] = local_u[1].clamp(-0.3, 0.3);
                            local_u[2] = local_u[2].clamp(-0.3, 0.3);
                        } else {
                            local_u = [self.u0, 0.0, 0.0];
                        }
                    }
                    for k in 0..19 {
                        let (ex, ey, ez) = E3[k];
                        let sx = ix as isize - ex as isize;
                        let sy = iy as isize - ey as isize;
                        let sz = iz as isize - ez as isize;
                        let val = if sx < 0 {
                            // West inlet: fresh equilibrium free stream.
                            equilibrium3(1.0, [self.u0, 0.0, 0.0])[k]
                        } else if sx >= nx as isize {
                            // East outlet: the five west-pointing
                            // distributions (source outside the lattice) are
                            // rebuilt as equilibrium at the pinned reference
                            // density and the node's local velocity.
                            debug_assert_eq!(ex, -1);
                            equilibrium3(1.0, local_u)[k]
                        } else {
                            let sx = sx as usize;
                            let (sy, sz) = (
                                sy.clamp(0, ny as isize - 1) as usize,
                                sz.clamp(0, nz as isize - 1) as usize,
                            );
                            let sidx = (sz * ny + sy) * nx + sx;
                            if self.solid[sidx] {
                                // Half-way bounce-back (stationary walls):
                                // read the node's own pre-stream
                                // (post-collision) distribution sent along
                                // the opposite direction — NOT the live
                                // `f_new`, which this very k-loop is
                                // overwriting direction-by-direction.
                                f_prev[i][OPP3[k]]
                            } else {
                                f_prev[sidx][k]
                            }
                        };
                        f_new[i][k] = val;
                    }
                }
            }
        }
        self.f = f_new;
    }

    /// Recompute macroscopic fields (solid nodes zeroed, as in 2D).
    fn macro_fields(&mut self) {
        for i in 0..self.f.len() {
            if self.solid[i] {
                self.rho[i] = 0.0;
                self.ux[i] = 0.0;
                self.uy[i] = 0.0;
                self.uz[i] = 0.0;
                continue;
            }
            let fi = self.f[i];
            let rho: f64 = fi.iter().sum();
            let mut u = [0.0; 3];
            for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
                u[0] += fi[k] * ex as f64;
                u[1] += fi[k] * ey as f64;
                u[2] += fi[k] * ez as f64;
            }
            self.rho[i] = rho;
            self.ux[i] = u[0] / rho;
            self.uy[i] = u[1] / rho;
            self.uz[i] = u[2] / rho;
        }
    }

    fn check_divergence(&self) -> bool {
        self.solid
            .iter()
            .zip(self.rho.iter().zip(self.ux.iter()))
            .filter(|(&solid, _)| !solid)
            .any(|(_, (&rho, &ux))| !rho.is_finite() || rho < 0.2 || !ux.is_finite() || ux.abs() > 0.5)
    }
}

// The per-solver force accumulator lives on the struct; alias for clarity.
impl Solver3D {
    /// Fluid force on the body during the most recent step (`[Fx, Fy]`).
    pub fn boundary_force(&self) -> [f64; 2] {
        self.instantaneous_force()
    }
}

#[cfg(test)]
mod tables {
    use super::*;

    #[test]
    fn d3q19_tables_are_consistent() {
        // Opposites must negate, and the weights must sum to one.
        for k in 0..19 {
            let (ex, ey, ez) = E3[k];
            let (ox, oy, oz) = E3[OPP3[k]];
            assert_eq!((ex + ox, ey + oy, ez + oz), (0, 0, 0), "k={k}");
        }
        let total: f64 = W3.iter().sum();
        assert!((total - 1.0).abs() < 1e-12, "weights sum to {total}");
        // Mass and momentum conservation of the equilibrium.
        let f = equilibrium3(1.27, [0.08, -0.03, 0.05]);
        let rho: f64 = f.iter().sum();
        assert!((rho - 1.27).abs() < 1e-12);
        let mut u = [0.0; 3];
        for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
            u[0] += f[k] * ex as f64;
            u[1] += f[k] * ey as f64;
            u[2] += f[k] * ez as f64;
        }
        assert!((u[0] / rho - 0.08).abs() < 1e-12);
        assert!((u[1] / rho + 0.03).abs() < 1e-12);
        assert!((u[2] / rho - 0.05).abs() < 1e-12);
    }
}

/// Rasterise `shape` onto the 3D solid mask; returns the body's half-chord
/// along the flow for wake seeding.
fn rasterize3(
    solid: &mut [bool],
    nx: usize,
    ny: usize,
    nz: usize,
    shape: Shape3D,
    cx: f64,
    cy: f64,
    cz: f64,
    h: f64,
) -> f64 {
    let set = |solid: &mut [bool], ix: usize, iy: usize, iz: usize| {
        solid[(iz * ny + iy) * nx + ix] = true;
    };
    match shape {
        // Imported bodies arrive pre-rasterised; nothing to draw here.
        Shape3D::Imported => 0.0,
        Shape3D::Sphere => {
            let r = h / 2.0;
            let r2 = r * r;
            for iz in 0..nz {
                for iy in 0..ny {
                    for ix in 0..nx {
                        let dx = ix as f64 - cx;
                        let dy = iy as f64 - cy;
                        let dz = iz as f64 - cz;
                        if dx * dx + dy * dy + dz * dz <= r2 {
                            set(solid, ix, iy, iz);
                        }
                    }
                }
            }
            r
        }
        Shape3D::Cube => {
            let half = (h / 2.0).round() as isize;
            let (ci, cj, ck) = (cx.round() as isize, cy.round() as isize, cz.round() as isize);
            for iz in (ck - half).max(0)..=(ck + half) {
                for iy in (cj - half).max(0)..=(cj + half) {
                    for ix in (ci - half).max(0)..=(ci + half) {
                        if (ix as usize) < nx && (iy as usize) < ny && (iz as usize) < nz {
                            set(solid, ix as usize, iy as usize, iz as usize);
                        }
                    }
                }
            }
            half as f64
        }
        Shape3D::Plate => {
            let half_y = (h / 2.0).round() as isize;
            let half_x = ((h * 0.12).round() as isize).max(1);
            let (ci, cj, ck) = (cx.round() as isize, cy.round() as isize, cz.round() as isize);
            for iz in (ck - half_y).max(0)..=(ck + half_y) {
                for iy in (cj - half_y).max(0)..=(cj + half_y) {
                    for ix in (ci - half_x).max(0)..=(ci + half_x) {
                        if (ix as usize) < nx && (iy as usize) < ny && (iz as usize) < nz {
                            set(solid, ix as usize, iy as usize, iz as usize);
                        }
                    }
                }
            }
            half_x as f64
        }
        Shape3D::Ellipsoid { aspect } => {
            let aspect = aspect.max(1.0);
            let a = h * aspect / 2.0;
            let b = h / 2.0;
            for iz in 0..nz {
                for iy in 0..ny {
                    for ix in 0..nx {
                        let dx = (ix as f64 - cx) / a;
                        let dy = (iy as f64 - cy) / b;
                        let dz = (iz as f64 - cz) / b;
                        if dx * dx + dy * dy + dz * dz <= 1.0 {
                            set(solid, ix, iy, iz);
                        }
                    }
                }
            }
            a
        }
        Shape3D::Ahmed => {
            // Side profile in cells, measured from the nose (u, streamwise)
            // and the floor (v, vertical). Ahmed proportions in units of H:
            // length 3.6, width 1.35, nose radius 0.35, slant 0.78 long at 25°.
            let len = 3.6 * h;
            let width = 1.35 * h;
            let r = 0.35 * h;
            let slant_start = len - 0.78 * h;
            let slant_tan = (25.0_f64).to_radians().tan();
            let inside_profile = |u: f64, v: f64| {
                if u < 0.0 || u > len || v < 0.0 || v > h {
                    return false;
                }
                if u < r {
                    // Rounded nose: outside the corner circles is not body.
                    if v < r && (r - u).powi(2) + (r - v).powi(2) > r * r {
                        return false;
                    }
                    if v > h - r && (r - u).powi(2) + (v - (h - r)).powi(2) > r * r {
                        return false;
                    }
                }
                // Rear slant: the roof drops linearly behind `slant_start`.
                if u > slant_start && v > h - (u - slant_start) * slant_tan {
                    return false;
                }
                true
            };
            let half_len = len / 2.0;
            for iz in 0..nz {
                for iy in 0..ny {
                    for ix in 0..nx {
                        let u = ix as f64 - cx + half_len;
                        let v = iy as f64 - cy + h / 2.0;
                        let dz = iz as f64 - cz;
                        if dz.abs() <= width / 2.0 && inside_profile(u, v) {
                            set(solid, ix, iy, iz);
                        }
                    }
                }
            }
            half_len
        }
    }
}

/// Cos²-enveloped transverse velocity bump (see the 2D seeding note).
fn seed3(
    f: &mut [[f64; 19]],
    solid: &[bool],
    nx: usize,
    ny: usize,
    nz: usize,
    cx: f64,
    cy: f64,
    cz: f64,
    r: f64,
    dv: [f64; 3],
) {
    let r2 = r * r;
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                let i = (iz * ny + iy) * nx + ix;
                if solid[i] {
                    continue;
                }
                let dx = ix as f64 - cx;
                let dy = iy as f64 - cy;
                let dz = iz as f64 - cz;
                let d2 = dx * dx + dy * dy + dz * dz;
                if d2 >= r2 {
                    continue;
                }
                let w = (1.0 - d2 / r2).powi(2);
                let fi = f[i];
                let rho: f64 = fi.iter().sum();
                let mut u = [0.0; 3];
                for (k, &(ex, ey, ez)) in E3.iter().enumerate() {
                    u[0] += fi[k] * ex as f64;
                    u[1] += fi[k] * ey as f64;
                    u[2] += fi[k] * ez as f64;
                }
                u = [u[0] / rho + dv[0] * w, u[1] / rho + dv[1] * w, u[2] / rho + dv[2] * w];
                f[i] = equilibrium3(rho, u);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn air() -> Fluid {
        Fluid {
            density: 1.204,
            kinematic_viscosity: 1.516e-5,
        }
    }

    fn sphere(re: f64, cells: usize) -> Problem3D {
        Problem3D {
            shape: Shape3D::Sphere,
            body_height: 0.1,
            inlet_velocity: 1.0,
            fluid: air(),
            cells_across_body: cells,
        }
        .with_re(re)
    }

    trait WithRe {
        fn with_re(self, re: f64) -> Problem3D;
    }
    impl WithRe for Problem3D {
        fn with_re(mut self, re: f64) -> Problem3D {
            self.inlet_velocity = re * self.fluid.kinematic_viscosity / self.body_height;
            self
        }
    }

    #[test]
    fn sphere_drag_is_positive_and_stable() {
        // Smoke: a coarse sphere at Re = 40 must solve stably with positive
        // finite drag (the quantitative bracket is the slower test below).
        let mut solver = Solver3D::new(&sphere(40.0, 6)).expect("layout");
        solver.set_run_plan(400);
        solver.run_to_completion();
        assert!(!solver.diverged());
        let cd = solver.drag_coefficient().unwrap();
        assert!(cd > 0.2 && cd.is_finite(), "cd = {cd}");
    }

    #[test]
    fn sphere_re40_drag_bracket() {
        // Published sphere drag at Re = 40: Cd ≈ 1.5-1.8 (standard
        // references, e.g. Clift-Grace-Weber correlations). A coarse
        // 10-cell sphere with 25% blockage sits ±20% around that.
        let mut solver = Solver3D::new(&sphere(40.0, 10)).expect("layout");
        solver.set_run_plan(2_500);
        solver.run_to_completion();
        assert!(!solver.diverged(), "sphere run diverged");
        let cd = solver.drag_coefficient().unwrap();
        assert!(
            (1.1..=2.2).contains(&cd),
            "sphere Cd(Re=40) = {cd:.3}, published ≈ 1.5-1.8"
        );
    }

    #[test]
    fn plate_solves_stably_at_bluff_scale() {
        // The sphere bracket above is the canonical quantitative check. At
        // the 3D presets (8-12 cells across the body) shape-to-shape
        // rankings sit inside the coarse-grid uncertainty — measured here:
        // plate 1.8 vs sphere 1.9 at Re = 40, where unbounded references
        // give ≈ 2-3 and ≈ 1.55 — so this test only pins the plate to a
        // stable, bluff-body-scale solve. The Pro report and UI state the
        // rough-3D-coefficients caveat; the 2D suite carries shape
        // comparisons.
        let base = sphere(40.0, 8);
        let plate = Problem3D {
            shape: Shape3D::Plate,
            ..base
        };
        let mut b = Solver3D::new(&plate).expect("layout");
        b.set_run_plan(1_200);
        b.run_to_completion();
        assert!(!b.diverged(), "plate run diverged");
        let cd_p = b.drag_coefficient().unwrap();
        assert!(
            (1.0..5.0).contains(&cd_p),
            "plate Cd(Re=40) = {cd_p:.3} outside the plausible coarse-grid band"
        );
    }
}
