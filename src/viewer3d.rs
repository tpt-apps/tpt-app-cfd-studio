//! Pro edition: 3D result viewer — two orthogonal slice heatmaps (the
//! mid-height XY plane and the mid-width XZ plane) drawn directly through
//! web-sys. The same colour ramp as the 2D viewer keeps the app coherent;
//! tracers are skipped (3D stepping is too coarse to animate smoothly).

use tpt_cfd_engine::threed::Solver3D;
use wasm_bindgen::{Clamped, JsCast, JsValue};

/// Logical canvas: two side-by-side slice panels plus a legend strip.
pub const W: f64 = 960.0;
pub const PANEL_H: f64 = 400.0;
pub const PANEL_W: f64 = 470.0;
pub const LEGEND_H: f64 = 44.0;
pub const H: f64 = PANEL_H + LEGEND_H;

pub struct Viewer3D {
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    off: web_sys::HtmlCanvasElement,
    off_ctx: web_sys::CanvasRenderingContext2d,
}

impl Viewer3D {
    pub fn attach(
        document: &web_sys::Document,
        holder: &web_sys::Element,
    ) -> Result<Viewer3D, JsValue> {
        let canvas: web_sys::HtmlCanvasElement = document
            .create_element("canvas")?
            .dyn_into()
            .map_err(|_| JsValue::from_str("3d canvas cast failed"))?;
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

        Ok(Viewer3D {
            canvas,
            ctx,
            off,
            off_ctx,
        })
    }

    /// Redraws both slices from the current 3D field.
    pub fn draw(&self, solver: &Solver3D) -> Result<(), JsValue> {
        let (nx, ny, nz) = (solver.nx, solver.ny, solver.nz);
        let u_ref = (1.8 * solver.info().u_lattice).max(1e-9);
        let mid_z = nz / 2;
        let mid_y = ny / 2;

        // Left panel: XY slice through the mid-height plane. Cell (ix, iy)
        // of the image samples the field at (ix, iy, mid_z).
        let xy = |ix: usize, iy: usize| -> (f64, f64, bool) {
            let i = mid_z * ny * nx + iy * nx + ix;
            (
                solver.ux[i],
                solver.uy[i],
                solver.solid[i],
            )
        };
        self.draw_panel(0.0, nx, ny, &xy, u_ref)?;

        // Right panel: XZ slice through the mid-width plane.
        let xz = |ix: usize, iz: usize| -> (f64, f64, bool) {
            let i = iz * ny * nx + mid_y * nx + ix;
            (
                solver.ux[i],
                solver.uz[i],
                solver.solid[i],
            )
        };
        self.draw_panel(PANEL_W + 20.0, nx, nz, &xz, u_ref)?;

        self.draw_legend()?;
        Ok(())
    }

    /// Renders one slice panel: `sample(ix, jy) -> (ux, u{y|z}, solid)` is
    /// the plane's field accessor over an `nx × nj` image.
    fn draw_panel(
        &self,
        x0: f64,
        nx: usize,
        nj: usize,
        sample: &dyn Fn(usize, usize) -> (f64, f64, bool),
        u_ref: f64,
    ) -> Result<(), JsValue> {
        let mut data = vec![0u8; nx * nj * 4];
        for (i, px) in data.chunks_exact_mut(4).enumerate() {
            let ix = i % nx;
            let jy = i / nx;
            let rgba: [u8; 4] = {
                let (ux, uv, solid) = sample(ix, jy);
                if solid {
                    [10, 14, 24, 255]
                } else {
                    let t = ((ux * ux + uv * uv).sqrt() / u_ref).clamp(0.0, 1.0);
                    let [r, g, b] = crate::viewer::speed_color(t);
                    [r, g, b, 255]
                }
            };
            px.copy_from_slice(&rgba);
        }
        self.off.set_width(nx as u32);
        self.off.set_height(nj as u32);
        let img = web_sys::ImageData::new_with_u8_clamped_array(Clamped(&data), nx as u32)?;
        self.off_ctx.put_image_data(&img, 0.0, 0.0)?;
        self.ctx.draw_image_with_html_canvas_element_and_dw_and_dh(
            &self.off,
            x0,
            0.0,
            PANEL_W,
            PANEL_H,
        )?;

        // Crisp solid-cell overlay for the body silhouette.
        let sx = PANEL_W / nx as f64;
        let sy = PANEL_H / nj as f64;
        self.ctx.set_fill_style_str("rgb(122,150,189)");
        for jy in 0..nj {
            for ix in 0..nx {
                let (_, _, solid) = sample(ix, jy);
                if solid {
                    let _ = self.ctx.fill_rect(
                        x0 + ix as f64 * sx,
                        jy as f64 * sy,
                        sx + 0.5,
                        sy + 0.5,
                    );
                }
            }
        }
        Ok(())
    }

    fn draw_legend(&self) -> Result<(), JsValue> {
        let ctx = &self.ctx;
        let y = PANEL_H + 14.0;
        let bar_x = 24.0;
        let bar_w = 340.0;
        let bar_h = 12.0;
        let strips = 68;
        let sw = bar_w / strips as f64;
        for s in 0..strips {
            let t = s as f64 / (strips - 1) as f64;
            let [r, g, b] = crate::viewer::speed_color(t);
            ctx.set_fill_style_str(&format!("rgb({r},{g},{b})"));
            let _ = ctx.fill_rect(bar_x + t * (bar_w - sw), y, sw + 0.5, bar_h);
        }
        ctx.set_fill_style_str("rgba(140,160,190,1)");
        ctx.set_font("11px system-ui, sans-serif");
        let _ = ctx.fill_text("0", bar_x, y + bar_h + 12.0);
        let _ = ctx.fill_text("1.8 U\u{221e} (lattice)", bar_x + bar_w - 96.0, y + bar_h + 12.0);
        let _ = ctx.fill_text(
            "left: top view (XY) · right: side view (XZ)",
            bar_x + bar_w + 18.0,
            y + bar_h,
        );
        Ok(())
    }
}
