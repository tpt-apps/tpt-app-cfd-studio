//! Perf budget probe (todo Phase 1): in-browser solve time is dominated by
//! lattice size × steps, so this measures host MLUPS on the free edition's
//! animated default layout and prints the projected wasm budget. Informational
//! only — run with `cargo test --release -p tpt-cfd-engine --test perf
//! -- --ignored` to check headroom.

use tpt_cfd_engine::{Fluid, Problem2D, Shape2D, Solver2D};

#[ignore] // informational: run with --ignored to check headroom
#[test]
fn free_edition_default_run_budget() {
    let problem = Problem2D {
        shape: Shape2D::Circle,
        body_height: 0.1,
        inlet_velocity: 1.0,
        fluid: Fluid {
            density: 1.204,
            kinematic_viscosity: 1.516e-5,
        },
        cells_across_body: 12,
    };

    // Lattice throughput: steps of the full free-edition layout, timed.
    let mut solver = Solver2D::new(&problem).expect("layout");
    let (nx, ny) = (solver.info().nx, solver.info().ny);
    let warm = std::time::Instant::now();
    solver.step_batch(2_000);
    let elapsed = warm.elapsed();
    let updates = 2_000.0 * (nx * ny) as f64;
    let mlups = updates / elapsed.as_secs_f64() / 1e6;

    // The UI animates the solve live (it is the show, not a blocking wait),
    // so the budget is the wall-clock a user watches at Quick resolution.
    // Modern wasm runs this scalar-f64 kernel within ~2x of the host.
    let full_run_steps = solver.planned_steps();
    let full_s = full_run_steps as f64 * (nx * ny) as f64 / (mlups * 1e6);

    println!("lattice {nx}x{ny} = {} cells", nx * ny);
    println!("host throughput: {mlups:.1} MLUPS (release build)");
    println!(
        "default run ({full_run_steps} steps): {full_s:.2}s host, ~{:.1}s in-browser (x2 wasm)",
        full_s * 2.0
    );
    assert!(
        full_s * 2.0 < 8.0,
        "free-tier default run projects to {:.2}s host (~x2 wasm) — over the \
         watch-it-develop budget; shrink the default plan or the Quick lattice",
        full_s
    );
}
