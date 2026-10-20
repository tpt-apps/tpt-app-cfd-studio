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
#[cfg(feature = "pro")]
use tpt_cfd_engine::threed::{Problem3D, Shape3D, Solver3D};
#[cfg(feature = "pro")]
use tpt_cfd_engine::stl::{self, Mesh, Orientation};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};

#[cfg(feature = "pro")]
use crate::viewer3d::Viewer3D;

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

/// (pro) 3D body shapes in picker order.
#[cfg(feature = "pro")]
const SHAPES_3D: [(&str, &str); 5] = [
    ("Sphere", "sphere"),
    ("Cube", "cube"),
    ("Flat plate", "plate"),
    ("Streamlined", "ellipsoid"),
    ("Ahmed body", "ahmed"),
];

/// (pro) 3D lattice presets: cells across the body. 3D cell counts explode
/// (n³ per direction pair), so these stay small; Fine ≈ 221k cells.
#[cfg(feature = "pro")]
const QUALITY_3D: [usize; 2] = [8, 12];

/// (pro) planned steps for the 3D default run (desktop wall-clock target:
/// tens of seconds with a live progress readout).
#[cfg(feature = "pro")]
const PLAN_STEPS_3D: usize = 3_500;

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
    /// (pro) pick a 3D body shape.
    #[cfg(feature = "pro")]
    Shape3D(&'static str),
    /// (pro) pick the 3D lattice preset (index into [`QUALITY_3D`]).
    #[cfg(feature = "pro")]
    Quality3D(usize),
    /// (pro) start (or stop) the 3D solve.
    #[cfg(feature = "pro")]
    Run3D,
    /// (pro) open the file picker and import an STL body for the 3D solve.
    #[cfg(feature = "pro")]
    ImportStl,
    /// (pro) cycle which model axis the flow runs along (X → Y → Z).
    #[cfg(feature = "pro")]
    CycleFlowAxis,
    /// (pro) reverse the flow direction along its axis.
    #[cfg(feature = "pro")]
    ReverseFlow,
    /// (pro) cycle which model axis is vertical.
    #[cfg(feature = "pro")]
    CycleUpAxis,
    /// (pro) download the current 2D flow view as a PNG.
    #[cfg(feature = "pro")]
    ExportPng,
    /// (pro) download the PDF report for the current solves.
    #[cfg(feature = "pro")]
    ExportReport,
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
    /// Adaptive stepping budget (lattice steps per animation-frame chunk).
    steps_per_frame: std::cell::Cell<usize>,
    last_batch: std::cell::Cell<(usize, f64)>,
    /// `Date.now()` of the last completed stepping chunk — the watchdog
    /// compares against this to detect a stalled animation loop.
    last_step_ms: std::cell::Cell<f64>,
    /// Watchdog interval id once installed.
    kick_id: std::cell::Cell<Option<i32>>,
    /// (pro) 3D solve state — the 3D card shares the panel's flow inputs.
    #[cfg(feature = "pro")]
    solver3d: RefCell<Option<Solver3D>>,
    #[cfg(feature = "pro")]
    viewer3d: RefCell<Option<Viewer3D>>,
    #[cfg(feature = "pro")]
    running3d: Rc<std::cell::Cell<bool>>,
    #[cfg(feature = "pro")]
    shape3d: std::cell::Cell<Shape3D>,
    #[cfg(feature = "pro")]
    quality3d: std::cell::Cell<usize>,
    /// (pro) the imported STL body, if one is loaded (its file name kept for
    /// the report and status lines).
    #[cfg(feature = "pro")]
    mesh3d: RefCell<Option<(String, Rc<Mesh>)>>,
    #[cfg(feature = "pro")]
    orient3d: std::cell::Cell<Orientation>,
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
        #[cfg(feature = "pro")]
        solver3d: RefCell::new(None),
        #[cfg(feature = "pro")]
        viewer3d: RefCell::new(None),
        #[cfg(feature = "pro")]
        running3d: Rc::new(std::cell::Cell::new(false)),
        #[cfg(feature = "pro")]
        shape3d: std::cell::Cell::new(Shape3D::Sphere),
        #[cfg(feature = "pro")]
        quality3d: std::cell::Cell::new(QUALITY_3D[0]),
        #[cfg(feature = "pro")]
        mesh3d: RefCell::new(None),
        #[cfg(feature = "pro")]
        orient3d: std::cell::Cell::new(Orientation::X_FLOW_Z_UP),
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
            #[cfg(feature = "pro")]
            Msg::Shape3D(id) => {
                // Picking a preset replaces any imported body.
                *app.mesh3d.borrow_mut() = None;
                app.shape3d.set(shape3d_from_id(id));
                patch_shape_buttons_by(&app.container, ".cfd-shape3", id);
                refresh_stl_labels(&app);
            }
            #[cfg(feature = "pro")]
            Msg::ImportStl => open_stl_picker(&app),
            #[cfg(feature = "pro")]
            Msg::CycleFlowAxis => {
                let mut o = app.orient3d.get();
                o.flow_axis = (o.flow_axis + 1) % 3;
                if o.up_axis == o.flow_axis {
                    o.up_axis = (o.up_axis + 1) % 3;
                }
                app.orient3d.set(o);
                refresh_stl_labels(&app);
            }
            #[cfg(feature = "pro")]
            Msg::ReverseFlow => {
                app.orient3d.set(app.orient3d.get().reversed());
                refresh_stl_labels(&app);
            }
            #[cfg(feature = "pro")]
            Msg::CycleUpAxis => {
                let mut o = app.orient3d.get();
                o.up_axis = (o.up_axis + 1) % 3;
                if o.up_axis == o.flow_axis {
                    o.up_axis = (o.up_axis + 1) % 3;
                }
                app.orient3d.set(o);
                refresh_stl_labels(&app);
            }
            #[cfg(feature = "pro")]
            Msg::Quality3D(q) => {
                app.quality3d.set(QUALITY_3D[q]);
                patch_buttons_by(&app.container, ".cfd-q3", q, &["Coarse", "Fine"]);
            }
            #[cfg(feature = "pro")]
            Msg::Run3D => {
                if app.running3d.get() {
                    stop_run3d(&app);
                } else {
                    start_3d_from_fields(&app);
                }
            }
            #[cfg(feature = "pro")]
            Msg::ExportPng => export_png(&app.container),
            #[cfg(feature = "pro")]
            Msg::ExportReport => export_report(&app),
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

    #[cfg(feature = "pro")]
    {
        let holder3 = container
            .query_selector(".cfd-canvas3-holder")?
            .ok_or_else(|| JsValue::from_str("3d canvas holder missing"))?;
        *app.viewer3d.borrow_mut() = Some(Viewer3D::attach(&document, &holder3)?);
        patch_shape_buttons_by(&app.container, ".cfd-shape3", "sphere");
        patch_buttons_by(&app.container, ".cfd-q3", 0, &["Coarse", "Fine"]);
    }

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
                    #[cfg(feature = "pro")]
                    row.text("pro · 2D + 3D").class("cfd-chip");
                    #[cfg(not(feature = "pro"))]
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

                #[cfg(feature = "pro")]
                {
                    pro_results_card(view);
                    pro_3d_card(view);
                }
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

/// (pro) Coefficient readout + export buttons card (view column).
#[cfg(feature = "pro")]
fn pro_results_card(view: &mut tpt_appfront_core::ContainerBuilder<Msg>) {
    view.container(|card| {
        card.container(|row| {
            row.text("Pro results").class("cfd-chip");
        })
        .class("cfd-upsell-head");
        card.heading(3, "Drag & lift coefficients").class("cfd-card-title");
        card.container(|grid| {
            grid.container(|cell| {
                cell.text("Drag coefficient Cd").class("cfd-label");
                cell.container(|l| {
                    l.text("—");
                })
                .class("cfd-coeff cfd-cd-value");
            })
            .class("cfd-coeff-cell");
            grid.container(|cell| {
                cell.text("Lift coefficient Cl").class("cfd-label");
                cell.container(|l| {
                    l.text("—");
                })
                .class("cfd-coeff cfd-cl-value");
            })
            .class("cfd-coeff-cell");
            grid.container(|cell| {
                cell.text("3D drag coefficient Cd").class("cfd-label");
                cell.container(|l| {
                    l.text("—");
                })
                .class("cfd-coeff cfd-cd3-value");
            })
            .class("cfd-coeff-cell");
        })
        .class("cfd-coeff-grid");
        card.container(|row| {
            row.button("Export PNG")
                .class("cfd-sample")
                .on_click(Msg::ExportPng);
            row.button("Export PDF report")
                .class("cfd-sample")
                .on_click(Msg::ExportReport);
        })
        .class("cfd-actions");
    })
    .class("cfd-card cfd-pro-card");
}

/// (pro) 3D external-flow card: shape picker, resolution, run button,
/// status lines, slice canvas. Shares the Flow card's speed/size inputs.
#[cfg(feature = "pro")]
fn pro_3d_card(view: &mut tpt_appfront_core::ContainerBuilder<Msg>) {
    view.container(|card| {
        card.heading(3, "3D external flow").class("cfd-card-title");
        card.container(|row| {
            for (label, id) in SHAPES_3D {
                row.button(label)
                    .class("cfd-shape3")
                    .attr("data-shape3", id)
                    .on_click(Msg::Shape3D(id));
            }
        })
        .class("cfd-shapes");
        card.container(|tools| {
            tools
                .button("Import STL…")
                .class("cfd-sample")
                .on_click(Msg::ImportStl);
            tools
                .button("Flow axis")
                .class("cfd-sample")
                .on_click(Msg::CycleFlowAxis);
            tools
                .button("Reverse flow")
                .class("cfd-sample")
                .on_click(Msg::ReverseFlow);
            tools
                .button("Up axis")
                .class("cfd-sample")
                .on_click(Msg::CycleUpAxis);
        })
        .class("cfd-3d-tools");
        card.container(|line| {
            line.text("Pick a preset shape, or import an STL of your own part.");
        })
        .class("cfd-stl-label");
        card.container(|line| {
            line.text("Flow: +X · Up: +Z");
        })
        .class("cfd-orient-label");
        card.container(|field| {
            field.text("3D resolution").class("cfd-label");
            field.container(|row| {
                row.button("Coarse").class("cfd-q3").on_click(Msg::Quality3D(0));
                row.button("Fine").class("cfd-q3").on_click(Msg::Quality3D(1));
            })
            .class("cfd-shapes");
        })
        .class("cfd-field");
        card.button("Run 3D solve")
            .class("cfd-run-btn cfd-run3d-btn")
            .on_click(Msg::Run3D);
        card.container(|status| {
            status
                .container(|line| {
                    line.text("3D — press Run to solve.");
                })
                .class("cfd-status3d-main");
            status
                .container(|line| {
                    line.text("Solves the same speed/size inputs in three dimensions.");
                })
                .class("cfd-status3d-note");
        })
        .class("cfd-status");
        card.container(|_| {})
            .class("cfd-canvas3-holder");
    })
    .class("cfd-card");
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
    if !any_running(app) {
        return;
    }
    let t0 = js_sys::Date::now();
    loop {
        if !any_running(app) {
            return;
        }
        // (pro) one 3D lattice step per chunk — a single 3D step costs as
        // much as ~30 2D steps at Fine resolution, so it gets its own slice
        // of every frame.
        #[cfg(feature = "pro")]
        if app.running3d.get() {
            let done3 = {
                let mut slot = app.solver3d.borrow_mut();
                match slot.as_mut() {
                    Some(solver) => {
                        solver.step_batch(1);
                        solver.is_complete()
                    }
                    None => true,
                }
            };
            if done3 {
                app.running3d.set(false);
                set_run3d_label(&app.container, false);
            }
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

/// `true` while any solve (2D, or Pro 3D) is still stepping.
#[cfg(not(feature = "pro"))]
fn any_running(app: &App) -> bool {
    app.running.get()
}

/// `true` while any solve (2D, or Pro 3D) is still stepping.
#[cfg(feature = "pro")]
fn any_running(app: &App) -> bool {
    app.running.get() || app.running3d.get()
}

/// Requests the next animation frame while a solve is running.
fn schedule_next_frame(app: &Rc<App>) {
    if !any_running(app) || app.raf.borrow().is_some() {
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
        if !any_running(&app2) {
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
    update_coefficients(app, Some(solver));
    #[cfg(feature = "pro")]
    update_3d(app);
}

/// (pro) redraws the 3D slices and the 3D status/coefficients.
#[cfg(feature = "pro")]
fn update_3d(app: &Rc<App>) {
    let mut slot = app.solver3d.borrow_mut();
    let Some(solver) = slot.as_mut() else {
        return;
    };
    if let Some(viewer) = app.viewer3d.borrow().as_ref() {
        let _ = viewer.draw(solver);
    }

    let info = solver.info();
    let progress = (solver.steps_run() as f64 / solver.planned_steps() as f64 * 100.0) as usize;
    let re = fmt_re(info.reynolds);
    let cap_note = if info.reynolds_capped {
        format!(
            "Target Re {} exceeds the 3D stable band — solving at Re {}.",
            fmt_re(solver.problem().reynolds()),
            re
        )
    } else if info.les {
        format!("Re = {} · turbulent (Smagorinsky LES)", re)
    } else {
        format!("Re = {}", re)
    };
    if solver.diverged() {
        set_status3d(
            &app.container,
            "3D solver diverged.",
            "Try the Coarse resolution or a lower speed, then run again.",
        );
    } else if app.running3d.get() {
        set_status3d(
            &app.container,
            &format!("3D solving — {}×{}×{} lattice · {}%", info.nx, info.ny, info.nz, progress.min(99)),
            &format!("{} · rough coefficients at this resolution", cap_note),
        );
    } else {
        set_status3d(
            &app.container,
            &format!(
                "3D done — {}×{}×{} lattice · {} steps",
                info.nx,
                info.ny,
                info.nz,
                fmt_re(solver.steps_run() as f64)
            ),
            &format!(
                "{} · 3D coefficients are rough estimates; 2D carries shape comparisons",
                cap_note
            ),
        );
    }

    let cd3 = match solver.drag_coefficient() {
        Some(cd) => format!("{cd:.3}"),
        None => "settling…".to_string(),
    };
    set_text(&app.container, ".cfd-cd3-value", &cd3);
}

/// (pro) writes the 2D time-averaged coefficients into the results card.
#[cfg(feature = "pro")]
fn update_coefficients(app: &Rc<App>, solver: Option<&Solver2D>) {
    let (cd, cl) = match solver {
        Some(solver) if solver.diverged() => ("diverged".to_string(), "—".to_string()),
        Some(solver) => match (solver.drag_coefficient(), solver.lift_coefficient()) {
            (Some(cd), Some(cl)) => (format!("{cd:.3}"), format!("{cl:+.3}")),
            _ => ("settling…".to_string(), "settling…".to_string()),
        },
        None => ("—".to_string(), "—".to_string()),
    };
    set_text(&app.container, ".cfd-cd-value", &cd);
    set_text(&app.container, ".cfd-cl-value", &cl);
}

#[cfg(not(feature = "pro"))]
fn update_coefficients(app: &Rc<App>, solver: Option<&Solver2D>) {
    let _ = (app, solver);
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

/// (pro) resolves a 3D shape picker id.
#[cfg(feature = "pro")]
fn shape3d_from_id(id: &str) -> Shape3D {
    match id {
        "cube" => Shape3D::Cube,
        "plate" => Shape3D::Plate,
        "ellipsoid" => Shape3D::Ellipsoid { aspect: 2.5 },
        "ahmed" => Shape3D::Ahmed,
        _ => Shape3D::Sphere,
    }
}

/// Highlights selected buttons by class prefix + attribute (shared by the
/// 2D and Pro pickers, which live in the mounted-once tree).
#[cfg(feature = "pro")]
fn patch_shape_buttons_by(container: &web_sys::Element, class: &str, selected: &str) {
    // `class` is a selector (".cfd-shape3"); the class attribute needs the bare name.
    let name = class.trim_start_matches('.');
    for (_, id) in SHAPES_3D {
        if let Some(Some(el)) = container
            .query_selector(&format!("{class}[data-shape3='{id}']"))
            .ok()
            .map(|o| o)
        {
            let cls = if id == selected {
                format!("{name} cfd-sel")
            } else {
                name.to_string()
            };
            let _ = el.set_attribute("class", &cls);
        }
    }
}

/// Highlights the selected option among a same-class button group.
#[cfg(feature = "pro")]
fn patch_buttons_by(container: &web_sys::Element, class: &str, selected: usize, labels: &[&str]) {
    // `class` is a selector (".cfd-q3"); the class attribute needs the bare name.
    let name = class.trim_start_matches('.');
    for (i, label) in labels.iter().enumerate() {
        if let Some(Some(el)) = container
            .query_selector(&format!("{class}:nth-of-type({})", i + 1))
            .ok()
            .map(|o| o)
        {
            let _ = el.set_text_content(Some(label));
            let cls = if i == selected {
                format!("{name} cfd-sel")
            } else {
                name.to_string()
            };
            let _ = el.set_attribute("class", &cls);
        }
    }
}

/// (pro) starts the 3D solve from the shared Flow inputs.
#[cfg(feature = "pro")]
fn start_3d_from_fields(app: &Rc<App>) {
    let read = |selector: &str| -> Option<f64> {
        app.container
            .query_selector(selector)
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .and_then(|input| input.value().trim().parse::<f64>().ok())
    };
    let (Some(speed), Some(size)) = (read(".cfd-inlet"), read(".cfd-size")) else {
        set_status3d(&app.container, "3D — check the Flow inputs first.", "");
        return;
    };
    if !(0.0001..=300.0).contains(&speed) || !(0.005..=20.0).contains(&size) {
        set_status3d(&app.container, "3D — speed/size are outside their ranges.", "");
        return;
    }
    let mesh = app.mesh3d.borrow().clone();
    let problem = Problem3D {
        shape: if mesh.is_some() {
            Shape3D::Imported
        } else {
            app.shape3d.get()
        },
        body_height: size,
        inlet_velocity: speed,
        fluid: Fluid::AIR,
        cells_across_body: app.quality3d.get(),
    };
    let built = match &mesh {
        Some((_, m)) => stl::voxelize(m, app.orient3d.get(), problem.cells_across_body).and_then(
            |body| {
                // A few unclosed columns are tolerable (they stay empty); a
                // lot means the file isn't a solid and the result would lie.
                if body.skipped_columns * 50 > body.covered_columns {
                    Err(tpt_cfd_engine::CfdError(format!(
                        "the STL is not watertight ({} of {} cross-section columns are open) — \
                         repair it in a mesh tool (MeshLab, Blender) and import again",
                        body.skipped_columns, body.covered_columns
                    )))
                } else {
                    Solver3D::from_voxels(&problem, body)
                }
            },
        ),
        None => Solver3D::new(&problem),
    };
    match built {
        Ok(solver) => {
            *app.solver3d.borrow_mut() = Some(solver);
            app.running3d.set(true);
            set_run3d_label(&app.container, true);
            app.tick.set(app.tick.get() + 1);
        }
        Err(error) => set_status3d(&app.container, "3D — cannot start:", &error.0),
    }
}

/// (pro) stops the 3D solve (finished or not).
#[cfg(feature = "pro")]
fn stop_run3d(app: &Rc<App>) {
    app.running3d.set(false);
    set_run3d_label(&app.container, false);
    app.tick.set(app.tick.get() + 1);
}

#[cfg(feature = "pro")]
fn set_run3d_label(container: &web_sys::Element, running: bool) {
    set_text(
        container,
        ".cfd-run3d-btn",
        if running { "Stop 3D" } else { "Run 3D solve" },
    );
}

#[cfg(feature = "pro")]
fn set_status3d(container: &web_sys::Element, main: &str, note: &str) {
    set_text(container, ".cfd-status3d-main", main);
    set_text(container, ".cfd-status3d-note", note);
}

#[cfg(feature = "pro")]
fn axis_name(axis: usize) -> &'static str {
    ["X", "Y", "Z"][axis.min(2)]
}

/// (pro) refreshes the STL / orientation lines under the 3D shape picker.
#[cfg(feature = "pro")]
fn refresh_stl_labels(app: &Rc<App>) {
    let o = app.orient3d.get();
    let sign = |s: f32| if s < 0.0 { '-' } else { '+' };
    set_text(
        &app.container,
        ".cfd-orient-label",
        &format!(
            "Flow: {}{} · Up: {}{}  (model axes)",
            sign(o.flow_sign),
            axis_name(o.flow_axis),
            sign(o.up_sign),
            axis_name(o.up_axis)
        ),
    );
    let label = match &*app.mesh3d.borrow() {
        Some((name, mesh)) => {
            let e = mesh.extent();
            format!(
                "STL: {name} · {} triangles · {:.3} × {:.3} × {:.3} model units (X×Y×Z). \
                 Scaled so its height equals the Body frontal height above.",
                mesh.triangle_count(),
                e[0],
                e[1],
                e[2]
            )
        }
        None => "Pick a preset shape, or import an STL of your own part.".to_string(),
    };
    set_text(&app.container, ".cfd-stl-label", &label);
}

/// (pro) opens the OS file picker; the chosen STL is read in the page and
/// handed to [`load_stl`].
#[cfg(feature = "pro")]
fn open_stl_picker(app: &Rc<App>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(input) = document
        .create_element("input")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().map_err(Into::into))
    else {
        return;
    };
    input.set_type("file");
    input.set_accept(".stl,model/stl,application/sla");
    let app = app.clone();
    let picked = input.clone();
    let on_change = Closure::once(move |_: web_sys::Event| {
        let Some(file) = picked.files().and_then(|files| files.get(0)) else {
            return;
        };
        let name = file.name();
        let Ok(reader) = web_sys::FileReader::new() else {
            return;
        };
        let done = reader.clone();
        let on_load = Closure::once(move |_: web_sys::Event| {
            if let Ok(buffer) = done.result() {
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                load_stl(&app, &name, &bytes);
            }
        });
        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        on_load.forget();
        let _ = reader.read_as_array_buffer(&file);
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
}

/// (pro) parses an STL and makes it the current 3D body.
#[cfg(feature = "pro")]
fn load_stl(app: &Rc<App>, name: &str, bytes: &[u8]) {
    match stl::parse_stl(bytes) {
        Ok(mesh) => {
            *app.mesh3d.borrow_mut() = Some((name.to_string(), Rc::new(mesh)));
            app.shape3d.set(Shape3D::Imported);
            patch_shape_buttons_by(&app.container, ".cfd-shape3", "");
            refresh_stl_labels(app);
            set_status3d(
                &app.container,
                "3D — STL loaded.",
                "Check the flow and up axes, then press Run 3D solve.",
            );
        }
        Err(error) => set_status3d(&app.container, "3D — cannot read that STL:", &error.0),
    }
}

/// Writes text content into the (container-wrapped) node at `selector`.
#[cfg(feature = "pro")]
fn set_text(container: &web_sys::Element, selector: &str, text: &str) {
    if let Some(Some(el)) = container.query_selector(selector).ok().map(|o| o) {
        el.set_text_content(Some(text));
    }
}

/// (pro) downloads the 2D flow canvas as a PNG.
#[cfg(feature = "pro")]
fn export_png(container: &web_sys::Element) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(Some(canvas)) = container.query_selector(".cfd-canvas").ok().map(|o| o) else {
        return;
    };
    let Ok(canvas) = canvas.dyn_into::<web_sys::HtmlCanvasElement>() else {
        return;
    };
    let callback = Closure::once(move |blob: Option<web_sys::Blob>| {
        if let Some(blob) = blob {
            download_blob(&document, &blob, "cfd-studio-flow.png");
        }
    });
    if canvas.to_blob(callback.as_ref().unchecked_ref()).is_ok() {
        callback.forget(); // ownership passes to the JS callback queue
    }
}

/// (pro) builds and downloads the PDF report for the current solves.
#[cfg(feature = "pro")]
fn export_report(app: &Rc<App>) {
    let rows = report_rows(app);
    let pdf = tpt_cfd_engine::report::render_pdf("TPT CFD Studio Report", &rows);
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let array = js_sys::Array::new();
    array.push(&js_sys::Uint8Array::from(pdf.as_slice()));
    match web_sys::Blob::new_with_u8_array_sequence(&array) {
        Ok(blob) => download_blob(&document, &blob, "cfd-studio-report.pdf"),
        Err(_) => (),
    }
}

/// Triggers a browser download of `blob` under `name`.
#[cfg(feature = "pro")]
fn download_blob(document: &web_sys::Document, blob: &web_sys::Blob, name: &str) {
    let Ok(url) = web_sys::Url::create_object_url_with_blob(blob) else {
        return;
    };
    if let Ok(anchor) = document.create_element("a") {
        let _ = anchor.set_attribute("href", &url);
        let _ = anchor.set_attribute("download", name);
        if let Some(body) = document.body() {
            let _ = body.append_child(&anchor);
        }
        if let Ok(a) = anchor.dyn_into::<web_sys::HtmlAnchorElement>() {
            a.click();
            if let Some(body) = document.body() {
                let _ = body.remove_child(&a);
            }
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// (pro) assembles the report rows from whatever has been solved.
#[cfg(feature = "pro")]
fn report_rows(app: &Rc<App>) -> Vec<tpt_cfd_engine::report::Row> {
    use tpt_cfd_engine::report::Row;

    let stamp = js_sys::Date::new_0().to_iso_string();
    let mut rows: Vec<Row> = vec![
        Row::Title("TPT CFD Studio — Flow Report".into()),
        Row::Note(format!(
            "Generated {stamp} · TPT CFD Studio Pro (lattice-Boltzmann external flow)"
        )),
        Row::Gap(6.0),
    ];

    {
        let slot = app.solver.borrow();
        if let Some(solver) = slot.as_ref() {
            let info = solver.info();
            let problem = solver.problem();
            let cap = if info.reynolds_capped {
                format!(
                    "{} (capped at the solver's stable ceiling)",
                    fmt_re(info.reynolds)
                )
            } else {
                fmt_re(info.reynolds)
            };
            rows.push(Row::Head("2D problem".into()));
            rows.push(Row::Table {
                columns: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["Body".into(), problem.shape.label().into()],
                    vec![
                        "Frontal height".into(),
                        format!("{:.3} m", problem.body_height),
                    ],
                    vec![
                        "Free-stream speed".into(),
                        format!("{:.3} m/s", problem.inlet_velocity),
                    ],
                    vec![
                        "Fluid".into(),
                        format!(
                            "air (rho = {:.3} kg/m3, nu = {:.3e} m2/s)",
                            problem.fluid.density, problem.fluid.kinematic_viscosity
                        ),
                    ],
                    vec!["Reynolds (target)".into(), fmt_re(problem.reynolds())],
                    vec!["Reynolds (simulated)".into(), cap],
                ],
                weights: Some(vec![0.35, 0.65]),
            });
            rows.push(Row::Gap(4.0));
            rows.push(Row::Head("2D results".into()));
            let (cd_text, cl_text);
            if solver.diverged() {
                cd_text = "diverged".to_string();
                cl_text = "-".to_string();
            } else if let (Some(cd), Some(cl)) =
                (solver.drag_coefficient(), solver.lift_coefficient())
            {
                cd_text = format!("{cd:.3}");
                cl_text = format!("{cl:+.3}");
            } else {
                cd_text = "not settled".to_string();
                cl_text = "not settled".to_string();
            }
            rows.push(Row::Table {
                columns: vec!["Quantity".into(), "Value".into()],
                rows: vec![
                    vec!["Drag coefficient Cd".into(), cd_text],
                    vec!["Lift coefficient Cl".into(), cl_text],
                    vec![
                        "Lattice".into(),
                        format!("{} x {} cells", info.nx, info.ny),
                    ],
                    vec!["Steps run".into(), fmt_re(solver.steps_run() as f64)],
                ],
                weights: Some(vec![0.45, 0.55]),
            });
        }
    }

    #[cfg(feature = "pro")]
    {
        let slot = app.solver3d.borrow();
        if let Some(solver) = slot.as_ref() {
            let info = solver.info();
            rows.push(Row::Gap(4.0));
            rows.push(Row::Head("3D results (rough estimates)".into()));
            rows.push(Row::Table {
                columns: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["Body".into(), solver.problem().shape.label().into()],
                    vec![
                        "Lattice".into(),
                        format!("{} x {} x {} cells", info.nx, info.ny, info.nz),
                    ],
                    vec!["Reynolds (simulated)".into(), fmt_re(info.reynolds)],
                    vec!["Steps run".into(), fmt_re(solver.steps_run() as f64)],
                    vec![
                        "Drag coefficient Cd".into(),
                        match solver.drag_coefficient() {
                            Some(cd) => format!("{cd:.3}"),
                            None => "not settled".into(),
                        },
                    ],
                ],
                weights: Some(vec![0.45, 0.55]),
            });
        }
    }

    rows.push(Row::Gap(4.0));
    rows.push(Row::Head("Method & accuracy".into()));
    rows.push(Row::Body(
        "Lattice-Boltzmann solver (BGK, D2Q9 in 2D / D3Q19 in 3D) with half-way \
         bounce-back bodies, a Zou-Hou style density-pinned outflow, and drag/lift \
         recovered by boundary momentum exchange, time-averaged over the second \
         half of the run."
         .into(),
    ));
    rows.push(Row::Body(
        "For the validated cylinder cases this reproduces published drag \
         coefficients to roughly 10-15%. Reynolds numbers above the BGK stability \
         ceiling are simulated at the ceiling and flagged, never silently. 3D \
         coefficients at the built-in resolutions are rough estimates; use the 2D \
         module for shape-to-shape comparisons."
         .into(),
    ));
    rows.push(Row::Note(
        "Not a substitute for engineering analysis or wind-tunnel testing.".into(),
    ));
    rows
}

fn visit_site(url: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };    let in_desktop_shell = window
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
:root{color-scheme:light dark;background:#f7f8fa}
body{margin:0;background:#f7f8fa}
@media(prefers-color-scheme:dark){:root,body{background:#0b0f16}}
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
.cfd-shape,.cfd-q,.cfd-shape3,.cfd-q3,.cfd-sample,.cfd-run-btn,.cfd-upsell-btn{font:inherit;font-weight:600;padding:8px 10px;border-radius:8px;border:1px solid var(--cfd-border);background:var(--cfd-panel);color:var(--cfd-fg);cursor:pointer;text-align:left}
.cfd-shape:hover,.cfd-q:hover,.cfd-shape3:hover,.cfd-q3:hover,.cfd-sample:hover{border-color:var(--cfd-accent)}
.cfd-3d-tools{display:grid;grid-template-columns:repeat(2,1fr);gap:8px;margin-top:10px}
.cfd-3d-tools .cfd-sample{margin-top:0}
.cfd-stl-label,.cfd-orient-label{color:var(--cfd-muted);font-size:.8rem;margin-top:6px}
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
