//! Problem/results types: the plain-struct contract between the UI (or the
//! desktop shell, or a test) and the solver.

/// Parametric 2D body shapes the picker offers. Every shape is sized by its
/// frontal height `H` (the projected height the flow sees), which doubles as
/// the characteristic length for Reynolds number and coefficients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape2D {
    /// Circular cylinder of diameter `H` — the canonical bluff-body
    /// validation case (vortex shedding above Re ≈ 47).
    Circle,
    /// Axis-aligned square of side `H`.
    Square,
    /// Flat plate normal to the flow: height `H`, thin rounded back/front.
    FlatPlate,
    /// Streamlined ellipse, height `H` across the flow and `aspect * H`
    /// along it (`aspect` ≥ 1; 4 gives the classic 4:1 streamlined body).
    Ellipse {
        /// Length-to-height ratio along the flow (clamped to ≥ 1).
        aspect: f64,
    },
}

impl Shape2D {
    /// Stable picker id (also the canvas button label key).
    pub fn id(self) -> &'static str {
        match self {
            Shape2D::Circle => "circle",
            Shape2D::Square => "square",
            Shape2D::FlatPlate => "plate",
            Shape2D::Ellipse { .. } => "ellipse",
        }
    }

    /// Human label for the picker and the report.
    pub fn label(self) -> &'static str {
        match self {
            Shape2D::Circle => "Cylinder",
            Shape2D::Square => "Square",
            Shape2D::FlatPlate => "Flat plate",
            Shape2D::Ellipse { .. } => "Streamlined body",
        }
    }
}

/// Working fluid.
#[derive(Debug, Clone, Copy)]
pub struct Fluid {
    /// Density ρ [kg/m³] (air ≈ 1.225 at sea level).
    pub density: f64,
    /// Kinematic viscosity ν [m²/s] (air ≈ 1.5e-5 at 20 °C).
    pub kinematic_viscosity: f64,
}

impl Fluid {
    /// Air at 20 °C, sea level — the app's default.
    pub const AIR: Fluid = Fluid {
        density: 1.204,
        kinematic_viscosity: 1.516e-5,
    };
}

/// One external-flow problem: a body in a uniform stream.
#[derive(Debug, Clone, Copy)]
pub struct Problem2D {
    pub shape: Shape2D,
    /// Frontal height of the body [m] (cylinder diameter, square side, …).
    pub body_height: f64,
    /// Free-stream velocity [m/s].
    pub inlet_velocity: f64,
    pub fluid: Fluid,
    /// Lattice cells across the body's frontal height — the resolution dial.
    /// (Coarse ≈ 16, Standard ≈ 24, Fine ≈ 40.)
    pub cells_across_body: usize,
}

impl Problem2D {
    /// Reynolds number of the problem, Re = U·H/ν.
    pub fn reynolds(&self) -> f64 {
        self.inlet_velocity * self.body_height / self.fluid.kinematic_viscosity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reynolds_is_velocity_times_height_over_viscosity() {
        let p = Problem2D {
            shape: Shape2D::Circle,
            body_height: 0.1,
            inlet_velocity: 3.0,
            fluid: Fluid::AIR,
            cells_across_body: 24,
        };
        let re = p.reynolds();
        let expected = 3.0 * 0.1 / 1.516e-5;
        assert!((re - expected).abs() < 1e-9 * expected, "re = {re}");
    }
}
