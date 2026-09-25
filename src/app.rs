//! Free WASM edition UI (tpt-appfront-dom), 2D qualitative flow.
//!
//! Rendering architecture (mirrors the fea-lite/cutlist-optimizer split):
//! - the **input panel is mounted once** (`tpt_appfront_dom::mount`) — typing
//!   never loses focus; field values are read straight from the DOM when Run
//!   is pressed;
//! - the **solver runs incrementally**: a `requestAnimationFrame` loop steps
//!   a few hundred lattice steps per frame (batch size adapts to hold ~10 ms
//!   of stepping per frame) and bumps a tick `Signal`;
//! - the **canvas viewer** is raw web-sys (the DOM backend has no canvas
//!   node kind), redrawn from an effect that watches the tick;
//! - the **status line** is two raw text nodes updated by the same effect,
//!   so per-frame updates never rebuild the UI tree.
//!
//! The free edition is deliberately qualitative: the flow field, live. Drag/
//! lift coefficients, the 3D solve, and exports are Pro (`--features pro`).

use std::{cell::RefCell, rc::Rc};

use tpt_appfront_core::{create_effect, Signal, UITree};
use tpt_cfd_engine::{Fluid, Problem2D, Shape2D, Solver2D};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};

use crate::viewer::Viewer;

const SITE_URL: &str = "https://tptsolutions.co.nz/tools/cfd-studio";

/// (label, `Shape2D::id()`) — order = button order in the panel.
const SHAPES: [(&str, &str); 4] = [
    ("Cylinder", "circle"),
    ("Square", "square"),
    ("Flat plate", "plate"),
    ("Streamlined", "ellipse"),
];

/// Lattice cells across the body: quick vs. standard quality. Quick (12)
/// keeps the animated default run inside a few seconds of browser time;
/// Standard (24) doubles the resolution for careful watching.
const QUALITY: [usize; 2] = [12, 24];

struct SampleDef {
    label: &'static str,
    shape: Shape2D,
    /// Free-stream speed [m/s] and body frontal height [m].
    speed: f64,
    size: f64,
    /// Lattice cells across the body: the sample's matching quality preset
    /// (the vortex-street sample needs Standard to hit its labelled Re).
    quality: usize,
}

/// Built-in samples: instant first-run value, one click each.
const SAMPLES: [SampleDef; 4] = [
    SampleDef {
        label: "Cylinder — steady wake (Re ≈ 20)",
        shape: Shape2D::Circle,
        speed: 0.003,
        size: 0.1,
        quality: 0,
    },
    SampleDef {
        label: "Cylinder — vortex street (Re ≈ 100)",
        shape: Shape2D::Circle,
        speed: 0.0152,
        size: 0.1,
        quality: 1,
    },
    SampleDef {
        label: "Streamlined fairing in crossflow",
        shape: Shape2D::Ellipse { aspect: 4.0 },
        speed: 0.02,
        size: 0.1,
        quality: 1,
    },
    SampleDef {
        label: "Drone arm — 8 m/s airflow",
        shape: Shape2D::Circle,
        speed: 8.0,
        size: 0.02,
        quality: 0,
    },
];

#[derive(Clone)]
enum Msg {
    /// Pick a body shape (id matches [`Shape2D::id`]).
    Shape(&'static str),
    /// Pick a lattice quality preset (index into [`QUALITY`]).
    Quality(usize),
    /// Load a built-in sample and start solving.
    Sample(usize),
    /// Start (or, while running, stop) the animated solve.
    Run,
    /// Open a TPT Solutions URL — new browser tab for the in-browser free
    /// edition; via the desktop host's system-browser IPC for the Pro
    /// desktop shell (whose webview must not navigate away from the local
    /// bundle). See [`visit_site`].
    VisitSite(&'static str),
}

struct App {
    container: web_sys::Element,
    solver: RefCell<Option<Solver2D>>,
    viewer: RefCell<Option<Viewer>>,
    /// Pending `requestAnimationFrame` handle + its closure (dropped on
    /// cancel so the callback chain never double-schedules).
    raf: RefCell<Option<(i32, Closure<dyn FnMut(f64)>)>>,
    tick: Signal<u64>,
    running: Rc<std::cell::Cell<bool>>,
    shape: std::cell::Cell<Shape2D>,
    quality: std::cell::Cell<usize>,
    /// Adaptive stepping budget (lattice steps per animation frame).
    steps_per_frame: std::cell::Cell<usize>,
    last_batch: std::cell::Cell<(usize, f64)>,
    /// `Date.now()` of the last completed stepping chunk — the watchdog
    /// compares against this to detect a stalled animation loop.
    last_step_ms: std::cell::Cell<f64>,
    /// Watchdog interval id once installed.
    kick_id: std::cell::Cell<Option<i32>>,
}

pub fn mount_app(container: &web_sys::Element) -> Result<(), JsValue> {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    upsert_style(&document, "cfd-studio-styles", BASE_CSS);
    // `.cfd-app` on the mount root itself: every color/font/shadow custom
    // property lives here, so the whole theme depends on this class being
    // present on an ancestor of the tree the CSS above targets.
    let existing_class = container.get_attribute("class").unwrap_or_default();
    if !existing_class.split_whitespace().any(|c| c == "cfd-app") {
        let combined = if existing_class.is_empty() {
            "cfd-app".to_string()
        } else {
            format!("{existing_class} cfd-app")
        };
        let _ = container.set_attribute("class", &combined);
    }

    let app = Rc::new(App {
        container: container.clone(),
        solver: RefCell::new(None),
        viewer: RefCell::new(None),
        raf: RefCell::new(None),
        tick: Signal::new(0),
        running: Rc::new(std::cell::Cell::new(false)),
        shape: std::cell::Cell::new(Shape2D::Circle),
        quality: std::cell::Cell::new(QUALITY[0]),
        steps_per_frame: std::cell::Cell::new(120),
        last_batch: std::cell::Cell::new((0, 0.0)),
        last_step_ms: std::cell::Cell::new(0.0),
        kick_id: std::cell::Cell::new(None),
    });

    let dispatch: Rc<dyn Fn(Msg)> = {
        let app = app.clone();
        Rc::new(move |msg| match msg {
            Msg::Shape(id) => {
                app.shape.set(shape_from_id(id));
                patch_shape_buttons(&app.container, id);
            }
            Msg::Quality(q) => {
                app.quality.set(QUALITY[q]);
                patch_quality_buttons(&app.container, q);
            }
            Msg::Sample(i) => start_sample(&app, i),
            Msg::Run => {
                if app.running.get() {
                    stop_run(&app);
                    set_run_label(&app.container, false);
                } else {
                    let _ = start_from_fields(&app);
                }
            }
            Msg::VisitSite(url) => visit_site(url),
        })
    };

    let ui = shell_tree();
    // The mount and every effect handle live for the whole process — the
    // signal system holds subscribers as `Weak`, so dropping a handle (or
    // the mount root) frees the effect node and silently kills reactivity
    // (fea-lite's `mem::forget` pattern for process-root apps).
    let root = tpt_appfront_dom::mount(container, &ui, dispatch)?;
    std::mem::forget(root);

    let holder = container
        .query_selector(".cfd-canvas-holder")?
        .ok_or_else(|| JsValue::from_str("canvas holder missing"))?;
    *app.viewer.borrow_mut() = Some(Viewer::attach(&document, &holder)?);

    // Draw/redraw on every tick; the effect also chains the next animation
    // frame while a solve is running.
    {
        let app = app.clone();
        let handle = create_effect(move || {
            app.tick.get();
            draw_frame(&app);
            schedule_next_frame(&app);
            ensure_kick_timer(&app);
        });
        std::mem::forget(handle);
    }

    // Instant first-run value: the vortex street is the showpiece.
    start_sample(&app, 1);
    Ok(())
}

/// The mounted-once shell: header, control panel, viewer, status, upsell.
fn shell_tree() -> UITree<Msg> {
    UITree::container(|c| {
        c.container(|header| {
            header.text("CFD").class("cfd-brand");
            header.container(|group| {
                group.container(|row| {
                    row.heading(2, "CFD Studio");
                    row.text("free · 2D qualitative").class("cfd-chip");
                });
                group
                    .text("Airflow around a body, solved live in your browser with a lattice-Boltzmann CFD engine.")
                    .class("cfd-sub");
            });
        })
        .class("cfd-header");

        c.container(|grid| {
            grid.container(|panel| {
                panel.container(|card| {
                    card.heading(3, "Body shape").class("cfd-card-title");
                    card.container(|row| {
                        for (label, id) in SHAPES {
                            row.button(label)
                                .class("cfd-shape")
                                .attr("data-shape", id)
                                .on_click(Msg::Shape(id));
                        }
                    })
                    .class("cfd-shapes");
                })
                .class("cfd-card");

                panel.container(|card| {
                    card.heading(3, "Flow").class("cfd-card-title");
                    card.container(|field| {
                        field.text("Free-stream speed [m/s]").class("cfd-label");
                        field.input("0.0152")
                            .class("cfd-input cfd-inlet")
                            .attr("type", "number")
                            .attr("step", "0.0001")
                            .attr("min", "0.0001")
                            .attr("max", "300");
                    })
                    .class("cfd-field");
                    card.container(|field| {
                        field.text("Body frontal height [m]").class("cfd-label");
                        field.input("0.1")
                            .class("cfd-input cfd-size")
                            .attr("type", "number")
                            .attr("step", "0.01")
                            .attr("min", "0.005")
                            .attr("max", "20");
                    })
                    .class("cfd-field");
                    card.container(|field| {
                        field.text("Resolution").class("cfd-label");
                        field.container(|row| {
                            row.button("Quick").class("cfd-q").on_click(Msg::Quality(0));
                            row.button("Standard").class("cfd-q").on_click(Msg::Quality(1));
                        })
                        .class("cfd-shapes");
                    })
                    .class("cfd-field");
                })
                .class("cfd-card");

                panel
                    .container(|row| {
                        row.button("Run").class("cfd-run-btn").on_click(Msg::Run);
                    })
                    .class("cfd-actions");

                panel.container(|card| {
                    card.heading(3, "Samples").class("cfd-card-title");
                    for (i, sample) in SAMPLES.iter().enumerate() {
                        card.button(sample.label)
                            .class("cfd-sample")
                            .on_click(Msg::Sample(i));
                    }
                })
                .class("cfd-card");

                #[cfg(not(feature = "pro"))]
                panel
                    .container(|up| {
                        up.container(|row| {
                            row.text("Pro").class("cfd-chip");
                        })
                        .class("cfd-upsell-head");
                        up.heading(3, "Go beyond qualitative").class("cfd-card-title");
                        up.text(
                            "The Pro desktop edition adds drag/lift coefficient output, a 3D \
                             external-flow solve, and exportable images and PDF reports — \
                             offline, one-time purchase.",
                        )
                        .class("cfd-upsell-copy");
                        up.button("See TPT CFD Studio Pro")
                            .class("cfd-upsell-btn")
                            .on_click(Msg::VisitSite(SITE_URL));
                    })
                    .class("cfd-upsell");
            })
            .class("cfd-panel");

            grid.container(|view| {
                view.container(|_| {})
                    .class("cfd-canvas-holder");
                // The status lines are containers, not bare text nodes: the
                // DOM backend renders bare `text()` children without the
                // requested class, and these two divs are what the per-frame
                // effect writes into (see `set_status`).
                view.container(|status| {
                    status
                        .container(|line| {
                            line.text("Press Run to solve.");
                        })
                        .class("cfd-status-main");
                    status
                        .container(|line| {
                            line.text(
                                "The flow develops live — watch the wake form behind the body.",
                            );
                        })
                        .class("cfd-status-note");
                })
                .class("cfd-status");
            })
            .class("cfd-view");
        })
        .class("cfd-grid");

        c.text("Qualitative 2D flow visualisation — not a substitute for engineering analysis. \
                Reynolds numbers above the solver's stable band are shown at the stable ceiling.")
            .class("cfd-footer");
    })
}

fn shape_from_id(id: &str) -> Shape2D {
    match id {
        "square" => Shape2D::Square,
        "plate" => Shape2D::FlatPlate,
        "ellipse" => Shape2D::Ellipse { aspect: 4.0 },
        _ => Shape2D::Circle,
    }
}

/// Reads the panel fields and (re)starts the animated solve.
fn start_from_fields(app: &Rc<App>) -> Result<(), JsValue> {
    let read = |selector: &str| -> Result<f64, String> {
        app.container
            .query_selector(selector)
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|input| input.value())
            .unwrap_or_default()
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("“{selector}” is not a number"))
    };

    let mut errors = Vec::new();
    let speed = read(".cfd-inlet").unwrap_or_else(|e| {
        errors.push(e);
        f64::NAN
    });
    let size = read(".cfd-size").unwrap_or_else(|e| {
        errors.push(e);
        f64::NAN
    });
    if errors.is_empty() {
        if !(0.0001..=300.0).contains(&speed) || !speed.is_finite() {
            errors.push("Speed must be between 0.0001 and 300 m/s.".into());
        }
        if !(0.005..=20.0).contains(&size) || !size.is_finite() {
            errors.push("Body height must be between 0.005 and 20 m.".into());
        }
    }
    if !errors.is_empty() {
        stop_run(app);
        set_status(
            &app.container,
            "Check the inputs:",
            &errors.join(" "),
        );
        return Err(JsValue::from_str("invalid input"));
    }

    let problem = Problem2D {
        shape: app.shape.get(),
        body_height: size,
        inlet_velocity: speed,
        fluid: Fluid::AIR,
        cells_across_body: app.quality.get(),
    };
    start_solve(app, problem)
}

/// Loads a sample's fields into the panel and starts its solve.
fn start_sample(app: &Rc<App>, index: usize) {
    let Some(sample) = SAMPLES.get(index) else {
        return;
    };
    app.shape.set(sample.shape);
    patch_shape_buttons(&app.container, sample.shape.id());
    app.quality.set(QUALITY[sample.quality]);
    patch_quality_buttons(&app.container, sample.quality);
    write_field(&app.container, ".cfd-inlet", &format!("{}", sample.speed));
    write_field(&app.container, ".cfd-size", &format!("{}", sample.size));
    let _ = start_from_fields(app);
}

/// Constructs the solver, resets the viewer, and kicks the animation loop.
fn start_solve(app: &Rc<App>, problem: Problem2D) -> Result<(), JsValue> {
    let solver = match Solver2D::new(&problem) {
        Ok(solver) => solver,
        Err(error) => {
            set_status(&app.container, "Cannot start the solve:", &error.0);
            return Err(JsValue::from_str(&error.0));
        }
    };
    let (nx, ny) = (solver.info().nx, solver.info().ny);
    if let Some(viewer) = app.viewer.borrow_mut().as_mut() {
        viewer.reset(nx, ny, solver.info().u_lattice, problem.inlet_velocity);
    }
    *app.solver.borrow_mut() = Some(solver);
    app.running.set(true);
    set_run_label(&app.container, true);
    app.tick.set(app.tick.get() + 1);
    Ok(())
}

fn stop_run(app: &Rc<App>) {
    app.running.set(false);
    if let Some((id, _closure)) = app.raf.borrow_mut().take() {
        if let Some(window) = web_sys::window() {
            let _ = window.cancel_animation_frame(id);
        }
    }
    set_run_label(&app.container, false);
}

/// One animation frame (or watchdog kick): advance the solver in adaptive
/// chunks until `budget_ms` of stepping is spent, then hand back to the tick
/// effect (draw + reschedule). The RAF path passes a ~10 ms budget; the
/// watchdog (below) passes a larger one when the animation loop is stalled.
fn do_frame(app: &Rc<App>, budget_ms: f64) {
    app.raf.borrow_mut().take();
    if !app.running.get() {
        return;
    }
    let t0 = js_sys::Date::now();
    loop {
        if !app.running.get() {
            return;
        }
        let steps = app.steps_per_frame.get();
        let chunk_t0 = js_sys::Date::now();
        let finished = {
            let mut slot = app.solver.borrow_mut();
            match slot.as_mut() {
                Some(solver) => {
                    solver.step_batch(steps);
                    solver.is_complete()
                }
                None => true,
            }
        };
        let chunk_dt = js_sys::Date::now() - chunk_t0;
        if chunk_dt > 0.5 {
            app.last_batch.set((steps, chunk_dt));
        }
        if chunk_dt > 1.0 {
            let next =
                ((steps as f64) * 10.0 / chunk_dt).round().clamp(24.0, 1500.0) as usize;
            app.steps_per_frame.set(next);
        }
        app.last_step_ms.set(js_sys::Date::now());
        if finished || app.solver.borrow().as_ref().is_none_or(|s| s.diverged()) {
            app.running.set(false);
            set_run_label(&app.container, false);
            break;
        }
        if js_sys::Date::now() - t0 >= budget_ms {
            break;
        }
    }
    app.tick.set(app.tick.get() + 1);
}

/// Requests the next animation frame while a solve is running.
fn schedule_next_frame(app: &Rc<App>) {
    if !app.running.get() || app.raf.borrow().is_some() {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let app2 = app.clone();
    let closure = Closure::wrap(Box::new(move |_: f64| {
        do_frame(&app2, 10.0);
    }) as Box<dyn FnMut(f64)>);
    match window.request_animation_frame(closure.as_ref().unchecked_ref()) {
        Ok(id) => *app.raf.borrow_mut() = Some((id, closure)),
        Err(_) => drop(closure),
    }
}

/// Installs (once) a 1.5 s watchdog: embedded contexts and background tabs
/// can throttle `requestAnimationFrame` to zero, which would freeze a
/// running solve forever. When no stepping has happened for >1.2 s while a
/// solve is running, the timer steps a time-boxed batch itself — timers
/// keep firing (clamped to ~1 Hz) where rAF does not.
fn ensure_kick_timer(app: &Rc<App>) {
    if app.kick_id.get().is_some() {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let app2 = app.clone();
    let closure = Closure::wrap(Box::new(move || {
        if !app2.running.get() {
            return;
        }
        if js_sys::Date::now() - app2.last_step_ms.get() > 1200.0 {
            do_frame(&app2, 40.0);
        }
    }) as Box<dyn FnMut()>);
    let id = window
        .set_interval_with_callback_and_timeout_and_arguments(
            closure.as_ref().unchecked_ref(),
            1500,
            &js_sys::Array::new(),
        )
        .ok();
    app.kick_id.set(id);
    // The interval lives for the whole process; the closure must outlive
    // this call, so leak it (the subscriber table holds it as `Weak`).
    std::mem::forget(closure);
}

fn draw_frame(app: &Rc<App>) {
    let mut slot = app.solver.borrow_mut();
    let Some(solver) = slot.as_mut() else {
        return;
    };
    if let Some(viewer) = app.viewer.borrow_mut().as_mut() {
        let _ = viewer.draw(solver, app.running.get(), app.steps_per_frame.get() as f64);
    }
    update_status(app, Some(solver));
}

fn update_status(app: &Rc<App>, solver: Option<&Solver2D>) {
    let Some(solver) = solver else {
        set_status(&app.container, "Press Run to solve.", "The flow develops live — watch the wake form behind the body.");
        return;
    };
    let info = solver.info();
    let re_target = solver.problem().reynolds();
    let re = fmt_re(info.reynolds);
    let cap_note = if info.reynolds_capped {
        format!(
            "Target Re {} is above the solver's stable band — showing the flow at Re {} instead.",
            fmt_re(re_target),
            re
        )
    } else {
        format!("Reynolds number Re = {}", re)
    };

    if solver.diverged() {
        set_status(
            &app.container,
            "Solver diverged — the flow went unstable.",
            "Reduce the speed, or switch Resolution to Quick, then Run again.",
        );
        return;
    }

    if app.running.get() {
        let (steps, dt) = app.last_batch.get();
        let rate = if dt > 0.0 { steps as f64 * 1000.0 / dt } else { 0.0 };
        let pct = (solver.steps_run() as f64 / solver.planned_steps() as f64 * 100.0) as usize;
        set_status(
            &app.container,
            &format!(
                "Solving — Re {} · {}×{} lattice · {}%",
                re,
                info.nx,
                info.ny,
                pct.min(99)
            ),
            &format!("{} · {:.0} lattice steps/s", cap_note, rate),
        );
    } else {
        set_status(
            &app.container,
            &format!(
                "Flow developed — Re {} · {}×{} lattice · {} steps",
                re,
                info.nx,
                info.ny,
                fmt_re(solver.steps_run() as f64)
            ),
            &cap_note,
        );
    }
}

fn set_status(container: &web_sys::Element, main: &str, note: &str) {
    if let Some(Some(el)) = container
        .query_selector(".cfd-status-main")
        .ok()
        .map(|o| o)
    {
        el.set_text_content(Some(main));
    }
    if let Some(Some(el)) = container.query_selector(".cfd-status-note").ok().map(|o| o) {
        el.set_text_content(Some(note));
    }
}

fn set_run_label(container: &web_sys::Element, running: bool) {
    if let Some(Some(el)) = container.query_selector(".cfd-run-btn").ok().map(|o| o) {
        el.set_text_content(Some(if running { "Stop" } else { "Run" }));
    }
}

fn write_field(container: &web_sys::Element, selector: &str, value: &str) {
    if let Some(Some(el)) = container.query_selector(selector).ok().map(|o| o) {
        if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
            input.set_value(value);
        }
    }
}

/// Highlights the selected shape/quality buttons (they live in the
/// mounted-once tree, so selection styling is patched in place).
fn patch_shape_buttons(container: &web_sys::Element, selected: &str) {
    for (_, id) in SHAPES {
        if let Some(Some(el)) = container
            .query_selector(&format!(".cfd-shape[data-shape='{id}']"))
            .ok()
            .map(|o| o)
        {
            let class = if id == selected {
                "cfd-shape cfd-sel"
            } else {
                "cfd-shape"
            };
            let _ = el.set_attribute("class", class);
        }
    }
}

fn patch_quality_buttons(container: &web_sys::Element, selected: usize) {
    for (i, label) in ["Quick", "Standard"].iter().enumerate() {
        if let Some(Some(el)) = container
            .query_selector(&format!(".cfd-q:nth-of-type({})", i + 1))
            .ok()
            .map(|o| o)
        {
            let _ = el.set_text_content(Some(label));
            let class = if i == selected { "cfd-q cfd-sel" } else { "cfd-q" };
            let _ = el.set_attribute("class", class);
        }
    }
}

fn visit_site(url: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let in_desktop_shell = window
        .location()
        .protocol()
        .map(|p| p == "app:")
        .unwrap_or(false);

    if in_desktop_shell {
        // Routed through the desktop host's allowlisted IPC bridge
        // (`window.__appfront.post`, injected by tpt-appfront-webview) so the
        // *native* process opens the OS default browser, leaving the app
        // window untouched. See `desktop/src/main.rs`'s `open_external`
        // handler.
        if let Ok(appfront) = js_sys::Reflect::get(&window, &JsValue::from_str("__appfront")) {
            if let Ok(post) = js_sys::Reflect::get(&appfront, &JsValue::from_str("post")) {
                if let Ok(post_fn) = post.dyn_into::<js_sys::Function>() {
                    let params = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &params,
                        &JsValue::from_str("url"),
                        &JsValue::from_str(url),
                    );
                    let _ =
                        post_fn.call2(&appfront, &JsValue::from_str("open_external"), &params);
                }
            }
        }
    } else {
        let _ = window.open_with_url_and_target(url, "_blank");
    }
}

/// `19 842`-style thousands separators for the Reynolds readout.
fn fmt_re(v: f64) -> String {
    let s = format!("{:.0}", v);
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 && ch != '-' {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    out
}

fn upsert_style(document: &web_sys::Document, id: &str, css: &str) {
    if let Some(existing) = document.get_element_by_id(id) {
        existing.set_text_content(Some(css));
        return;
    }
    if let Some(head) = document.head() {
        if let Ok(style) = document.create_element("style") {
            let _ = style.set_attribute("id", id);
            style.set_text_content(Some(css));
            let _ = head.append_child(&style);
        }
    }
}

const BASE_CSS: &str = r#"
.cfd-app{--cfd-bg:#f7f8fa;--cfd-surface:#ffffff;--cfd-panel:#f2f4f7;--cfd-fg:#181d27;--cfd-muted:#667085;--cfd-border:#e3e7ee;--cfd-accent:#0e7c6b;--cfd-accent-2:#0a5f52;--cfd-accent-soft:#e6f5f2;--cfd-accent-fg:#ffffff;--cfd-err:#b3261e;max-width:1360px;margin:0 auto;background:var(--cfd-bg);color:var(--cfd-fg);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;font-size:14.5px;line-height:1.55;padding:0 4px 32px}
@media(prefers-color-scheme:dark){.cfd-app{--cfd-bg:#0b0f16;--cfd-surface:#141a24;--cfd-panel:#161d2a;--cfd-fg:#e8ebf0;--cfd-muted:#98a2b3;--cfd-border:#262f3d;--cfd-accent:#34b39a;--cfd-accent-2:#4fd6ba;--cfd-accent-soft:#122824;--cfd-accent-fg:#04140f}}
.cfd-app *{box-sizing:border-box}
.cfd-app h2{font-size:1.36rem;margin:0;font-weight:700;letter-spacing:-.01em}
.cfd-app h3{font-size:1rem;margin:0 0 6px;font-weight:700}
.cfd-header{display:flex;align-items:flex-start;gap:14px;padding:22px 2px 18px;border-bottom:1px solid var(--cfd-border);margin-bottom:22px}
.cfd-brand{flex:0 0 auto;width:42px;height:42px;border-radius:11px;background:linear-gradient(135deg,var(--cfd-accent),var(--cfd-accent-2));display:flex;align-items:center;justify-content:center;color:var(--cfd-accent-fg);font-weight:800;font-size:.82rem;box-shadow:0 1px 3px rgba(16,24,40,.12)}
.cfd-title-group{flex:1;min-width:0}
.cfd-title-row{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.cfd-chip{display:inline-flex;align-items:center;padding:3px 10px;border-radius:999px;font-size:.66rem;font-weight:800;letter-spacing:.05em;text-transform:uppercase;background:var(--cfd-accent-soft);color:var(--cfd-accent)}
.cfd-sub{color:var(--cfd-muted);margin:5px 0 0;font-size:.88rem}
.cfd-grid{display:grid;grid-template-columns:330px minmax(0,1fr);gap:18px;align-items:start}
@media(max-width:900px){.cfd-grid{grid-template-columns:1fr}}
.cfd-panel{display:flex;flex-direction:column;gap:14px}
.cfd-card{background:var(--cfd-surface);border:1px solid var(--cfd-border);border-radius:12px;padding:14px 16px;box-shadow:0 1px 2px rgba(16,24,40,.04)}
.cfd-card-title{margin-bottom:10px}
.cfd-label{display:block;font-size:.78rem;font-weight:600;color:var(--cfd-muted);margin:10px 0 4px}
.cfd-field:first-child .cfd-label{margin-top:0}
.cfd-input{width:100%;padding:8px 10px;border:1px solid var(--cfd-border);border-radius:8px;background:var(--cfd-surface);color:var(--cfd-fg);font:inherit}
.cfd-input:focus{outline:2px solid var(--cfd-accent);outline-offset:-1px}
.cfd-shapes{display:grid;grid-template-columns:1fr 1fr;gap:8px}
.cfd-shape,.cfd-q,.cfd-sample,.cfd-run-btn,.cfd-upsell-btn{font:inherit;font-weight:600;padding:8px 10px;border-radius:8px;border:1px solid var(--cfd-border);background:var(--cfd-panel);color:var(--cfd-fg);cursor:pointer;text-align:left}
.cfd-shape:hover,.cfd-q:hover,.cfd-sample:hover{border-color:var(--cfd-accent)}
.cfd-sel{border-color:var(--cfd-accent);background:var(--cfd-accent-soft);color:var(--cfd-accent);font-weight:700}
.cfd-sample{margin-top:8px;width:100%}
.cfd-actions{display:flex;gap:10px}
.cfd-run-btn{flex:1;text-align:center;background:var(--cfd-accent);border-color:var(--cfd-accent);color:var(--cfd-accent-fg);font-weight:700}
.cfd-run-btn:hover{background:var(--cfd-accent-2)}
.cfd-view{background:var(--cfd-surface);border:1px solid var(--cfd-border);border-radius:12px;padding:12px;box-shadow:0 1px 2px rgba(16,24,40,.04)}
.cfd-canvas-holder{border-radius:8px;overflow:hidden;background:#0a0e18}
.cfd-canvas{display:block;width:100%;height:auto}
.cfd-status{padding:10px 4px 2px}
.cfd-status-main{font-weight:700}
.cfd-status-note{color:var(--cfd-muted);font-size:.85rem;margin-top:2px}
.cfd-upsell{border:1px solid var(--cfd-accent);background:var(--cfd-accent-soft);border-radius:12px;padding:14px 16px}
.cfd-upsell-head{margin-bottom:4px}
.cfd-upsell-copy{color:var(--cfd-fg);font-size:.86rem;margin:0 0 10px}
.cfd-upsell-btn{width:100%;text-align:center;background:var(--cfd-accent);border-color:var(--cfd-accent);color:var(--cfd-accent-fg)}
.cfd-footer{color:var(--cfd-muted);font-size:.78rem;margin:18px 2px 0}
"#;
