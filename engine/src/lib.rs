//! tpt-cfd-engine — the simulation core of TPT CFD Studio.
//!
//! A thin, UI-free adapter over [tpt-phys-cfd]'s D2Q9 lattice-Boltzmann
//! solver: it turns a physical external-flow problem (body shape, frontal
//! height, inlet velocity, fluid) into a lattice setup, steps it (or drives
//! it incrementally so a browser UI can animate the flow developing), and
//! recovers drag/lift coefficients via boundary momentum exchange. Behind the
//! `pro` feature sit the 3D D3Q19 solver and the PDF report writer.
//!
//! Everything here is plain `std` Rust with no wasm/UI dependencies, so the
//! correctness moat is host-testable: `cargo test -p tpt-cfd-engine` runs
//! published-reference validation cases (cylinder in crossflow Cd vs.
//! Reynolds number, normal flat plate) before any browser is involved.
//!
//! [tpt-phys-cfd]: https://github.com/tpt-solutions/tpt-physics

pub mod model;
pub mod solve2d;

pub use model::{Fluid, Problem2D, Shape2D};
pub use solve2d::{CfdError, LatticeInfo, Solver2D};
