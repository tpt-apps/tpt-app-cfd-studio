//! Canvas flow viewer: live velocity-magnitude field with tracer
//! particles, drawn directly through web-sys (the DOM backend has no canvas
//! node kind, so the `<canvas>` is created manually and appended into a
//! placeholder container from the app shell).
//!
//! The lattice field is rendered once per solve-layout into an offscreen
//! `nx×ny` canvas and scaled up with smoothing for a soft heatmap look;
//! tracers are advected through the current field and drawn as fading
//! streaks, which makes the vortex street readable at a glance.

use tpt_cfd_engine::Solver2D;
use tpt_phys_cfd::Lbm2D;
use wasm_bindgen::{Clamped, JsCast, JsValue};

/// Logical canvas size (CSS scales it to the card width). The field area is
/// sized for the 12h×5h lattice aspect (2.4:1) plus a legend strip below.
pub const W: f64 = 960.0;
pub const FIELD_H: f64 = 400.0;
pub const LEGEND_H: f64 = 44.0;
pub const H: f64 = FIELD_H + LEGEND_H;

/// Tracer particles advected through the field.
const PARTICLES: usize = 420;

pub struct Viewer {
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    off: web_sys::HtmlCanvasElement,
    off_ctx: web_sys::CanvasRenderingContext2d,
    /// Tracer positions in lattice coordinates `[x, y]` and their previous
    /// positions (for the streak segments).
    particles: Vec<[[f64; 2]; 2]>,
    /// Free-stream colour-map scale and physical speed for the legend.
    speed_scale: f64,
    speed_max_mps: f64,
}

impl Viewer {
    /// Creates the canvas and appends it into `holder`.
    pub fn attach(document: &web_sys::Document, holder: &web_sys::Element) -> Result<Viewer, JsValue> {
        let canvas: web_sys::HtmlCanvasElement = document
            .create_element("canvas")?
            .dyn_into()
            .map_err(|_| JsValue::from_str("canvas cast failed"))?;
        canvas.set_width(W as u32);
        canvas.set_height(H as u32);
        let _ = canvas.set_attribute("class", "cfd-canvas");
        holder.append_child(&canvas)?;

        let off: web_sys::HtmlCanvasElement = document
            .create_element("canvas")?
            .dyn_into()
            .map_err(|_| JsValue::from_str("offscreen canvas cast failed"))?;
        let ctx: web_sys::CanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("no 2d context"))?
            .dyn_into()
            .map_err(|_| JsValue::from_str("2d context cast failed"))?;
        let off_ctx: web_sys::CanvasRenderingContext2d = off
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("no offscreen 2d context"))?
            .dyn_into()
            .map_err(|_| JsValue::from_str("offscreen 2d context cast failed"))?;

        Ok(Viewer {
            canvas,
            ctx,
            off,
            off_ctx,
            particles: Vec::new(),
            speed_scale: 1.0,
            speed_max_mps: 1.0,
        })
    }

    /// Prepares the viewer for a new lattice layout: resizes the offscreen
    /// field buffer and scatters fresh tracers over the upstream half.
    pub fn reset(&mut self, nx: usize, ny: usize, u_lattice: f64, u_mps: f64) {
        self.off.set_width(nx as u32);
        self.off.set_height(ny as u32);
        self.speed_scale = 1.8 * u_lattice;
        self.speed_max_mps = 1.8 * u_mps;
        let mut state = 0x2545_f491_4f6c_dd1d_u64; // xorshift state
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        self.particles = (0..PARTICLES)
            .map(|_| {
                let p = [1.0 + rand() * (nx as f64 * 0.45), rand() * (ny as f64 - 2.0) + 1.0];
                [p, p]
            })
            .collect();
    }

    /// Redraws one frame: field heatmap, tracer streaks, body, legend.
    pub fn draw(&mut self, solver: &Solver2D, advance: bool, adv_steps: f64) -> Result<(), JsValue> {
        let lat = solver.lattice();
        let (nx, ny) = (lat.nx, lat.ny);
        let scale = self.speed_scale.max(1e-9);

        // 1. Field heatmap on the offscreen lattice-sized canvas.
        let mut data = vec![0u8; nx * ny * 4];
        for (i, px) in data.chunks_exact_mut(4).enumerate() {
            let rgba: [u8; 4] = if lat.solid[i] {
                SOLID
            } else {
                let sp =
                    (lat.ux[i] * lat.ux[i] + lat.uy[i] * lat.uy[i]).sqrt() / scale;
                let t = sp.clamp(0.0, 1.0);
                let [r, g, b] = speed_color(t);
                [r, g, b, 255]
            };
            px.copy_from_slice(&rgba);
        }
        let img = web_sys::ImageData::new_with_u8_clamped_array(Clamped(&data), nx as u32)?;
        self.off_ctx.put_image_data(&img, 0.0, 0.0)?;

        // 2. Scale the lattice field onto the display canvas (smoothing on).
        self.ctx
            .draw_image_with_html_canvas_element_and_dw_and_dh(
                &self.off,
                0.0,
                0.0,
                W,
                FIELD_H,
            )?;

        // 3. Tracer streaks, advected through the current field.
        if advance && !self.particles.is_empty() {
            let dt = adv_steps.max(1.0) * 0.12;
            for p in &mut self.particles {
                let [x, y] = p[0];
                let (u, v) = sample_velocity(lat, x, y);
                let mut next = [x + u * dt, y + v * dt];
                // Respawn tracers that leave, sit in the body, or have run
                // long enough: recirculation keeps some alive forever.
                let in_solid = solid_at(lat, next[0], next[1]);
                let wandered_off = next[0] < 0.5 || next[0] > nx as f64 - 1.5;
                let out_y = next[1] < 0.5 || next[1] > ny as f64 - 1.5;
                if in_solid || wandered_off || out_y {
                    next = [1.0, y]; // re-enter at the inlet edge
                    p[1] = next;
                    p[0] = next;
                    continue;
                }
                p[1] = p[0];
                p[0] = next;
            }
            self.draw_particles(nx, ny);
        }

        // 4. Crisp body outline on top of the smoothed field.
        self.draw_body(lat)?;

        // 5. Legend strip.
        self.draw_legend();

        Ok(())
    }

    fn draw_particles(&self, nx: usize, ny: usize) {
        let ctx = &self.ctx;
        let sx = W / nx as f64;
        let sy = FIELD_H / ny as f64;
        ctx.set_stroke_style_str("rgba(255,255,255,0.55)");
        ctx.set_line_width(1.0);
        ctx.begin_path();
        for p in &self.particles {
            let [x0, y0] = p[1];
            let [x1, y1] = p[0];
            ctx.move_to(x0 * sx, y0 * sy);
            ctx.line_to(x1 * sx, y1 * sy);
        }
        ctx.stroke();
    }

    /// Body cells drawn cell-by-cell over the smoothed field so the shape
    /// edges stay crisp; a 1-cell lighter rim marks the surface.
    fn draw_body(&self, lat: &tpt_phys_cfd::Lbm2D) -> Result<(), JsValue> {
        let (nx, ny) = (lat.nx, lat.ny);
        let sx = W / nx as f64;
        let sy = FIELD_H / ny as f64;
        let solid_at = |ix: usize, iy: usize| lat.solid[iy * nx + ix];
        for iy in 0..ny {
            for ix in 0..nx {
                if !solid_at(ix, iy) {
                    continue;
                }
                self.ctx.set_fill_style_str("rgb(13,20,33)");
                let _ = self.ctx.fill_rect(
                    ix as f64 * sx,
                    iy as f64 * sy,
                    sx + 0.5,
                    sy + 0.5,
                );
                let on_surface = ix == 0
                    || iy == 0
                    || ix == nx - 1
                    || iy == ny - 1
                    || !solid_at(ix - 1, iy)
                    || !solid_at(ix + 1, iy)
                    || !solid_at(ix, iy - 1)
                    || !solid_at(ix, iy + 1);
                if on_surface {
                    self.ctx.set_fill_style_str("rgb(122,150,189)");
                    let _ = self.ctx.fill_rect(
                        ix as f64 * sx,
                        iy as f64 * sy,
                        sx + 0.5,
                        1.2,
                    );
                }
            }
        }
        Ok(())
    }

    /// Legend: colour bar sampled from the same ramp as the field, with the
    /// speed scale labelled in physical units.
    fn draw_legend(&self) {
        let ctx = &self.ctx;
        let y = FIELD_H + 14.0;
        let bar_x = 24.0;
        let bar_w = 340.0;
        let bar_h = 12.0;
        let strips = 68;
        let sw = bar_w / strips as f64;
        for s in 0..strips {
            let t = s as f64 / (strips - 1) as f64;
            let [r, g, b] = speed_color(t);
            ctx.set_fill_style_str(&format!("rgb({r},{g},{b})"));
            let _ = ctx.fill_rect(bar_x + t * (bar_w - sw), y, sw + 0.5, bar_h);
        }
        ctx.set_fill_style_str("rgba(140,160,190,1)");
        ctx.set_font("11px system-ui, sans-serif");
        let _ = ctx.fill_text("0", bar_x, y + bar_h + 12.0);
        let _ = ctx.fill_text(
            &format!("{:.2} m/s", self.speed_max_mps),
            bar_x + bar_w - 52.0,
            y + bar_h + 12.0,
        );
        let _ = ctx.fill_text("flow speed", bar_x + bar_w + 18.0, y + bar_h);
        let _ = self
            .ctx
            .fill_text("", self.canvas.width() as f64 - 10.0, y + bar_h);
    }
}

const SOLID: [u8; 4] = [10, 14, 24, 255];

/// Bilinear velocity sample in lattice coordinates; solid cells read as
/// still fluid (tracers stall at the surface and respawn).
fn sample_velocity(lat: &Lbm2D, x: f64, y: f64) -> (f64, f64) {
    let nx = lat.nx as f64;
    let ny = lat.ny as f64;
    let x = x.clamp(0.0, nx - 1.001);
    let y = y.clamp(0.0, ny - 1.001);
    let ix = x as usize;
    let iy = y as usize;
    let fx = x - ix as f64;
    let fy = y - iy as usize as f64;
    let at = |ix: usize, iy: usize| -> (f64, f64) {
        if lat.solid[iy * lat.nx + ix] {
            (0.0, 0.0)
        } else {
            let i = iy * lat.nx + ix;
            (lat.ux[i], lat.uy[i])
        }
    };
    let (u00, v00) = at(ix, iy);
    let (u10, v10) = at(ix + 1, iy);
    let (u01, v01) = at(ix, iy + 1);
    let (u11, v11) = at(ix + 1, iy + 1);
    let u = u00 * (1.0 - fx) * (1.0 - fy)
        + u10 * fx * (1.0 - fy)
        + u01 * (1.0 - fx) * fy
        + u11 * fx * fy;
    let v = v00 * (1.0 - fx) * (1.0 - fy)
        + v10 * fx * (1.0 - fy)
        + v01 * (1.0 - fx) * fy
        + v11 * fx * fy;
    (u, v * fy.signum())
}

fn solid_at(lat: &Lbm2D, x: f64, y: f64) -> bool {
    let ix = x as usize;
    let iy = y as usize;
    ix < lat.nx && iy < lat.ny && lat.solid[iy * lat.nx + ix]
}

/// Deep-navy → steel-blue → pale-cyan ramp; the free stream sits at ~55% so
/// wake deficits read clearly against it. Shared with the Pro 3D viewer.
pub(crate) fn speed_color(t: f64) -> [u8; 3] {
    let stops: [(f64, [u8; 3]); 4] = [
        (0.0, [9, 14, 28]),
        (0.4, [30, 78, 125]),
        (0.7, [96, 160, 213]),
        (1.0, [205, 236, 255]),
    ];
    for w in stops.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = (t - t0) / (t1 - t0);
            return [
                (c0[0] as f64 + f * (c1[0] - c0[0]) as f64) as u8,
                (c0[1] as f64 + f * (c1[1] - c0[1]) as f64) as u8,
                (c0[2] as f64 + f * (c1[2] - c0[2]) as f64) as u8,
            ];
        }
    }
    stops[stops.len() - 1].1
}
