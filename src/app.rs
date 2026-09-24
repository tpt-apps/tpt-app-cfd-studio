//! Free WASM edition UI (tpt-appfront-dom).
//!
//! Phase 0 scaffold: a header shell proving the hub-contract pipeline end to
//! end (wasm-bindgen `--target web` glue, `mount(container)` entry, marker-div
//! standalone mount). The input panel / solver / canvas viewer land in Phase 2
//! with the same architecture as fea-lite: input panel mounted once, results
//! re-rendered fine-grained from a Signal, canvas driven by raw web-sys.

use std::rc::Rc;

use tpt_appfront_core::UITree;
use wasm_bindgen::prelude::*;

/// App messages. Phase 0 has no interactive controls yet, so this is empty;
/// Phase 2 fills it in (solve, shape pick, sample, open site).
#[derive(Clone)]
pub enum Msg {}

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

    let ui = UITree::container(|b| {
        b.heading(2, "TPT CFD Studio").class("cfd-title");
        b.text("2D airflow around a body — solver integration in progress.").class("cfd-sub");
    });
    let dispatch: Rc<dyn Fn(Msg)> = Rc::new(|_| {});
    let _root = tpt_appfront_dom::mount(container, &ui, dispatch)?;
    Ok(())
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
.cfd-app{--cfd-bg:#f7f8fa;--cfd-fg:#181d27;--cfd-muted:#667085;max-width:1360px;margin:0 auto;background:var(--cfd-bg);color:var(--cfd-fg);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;font-size:14.5px;line-height:1.55;padding:0 4px 32px}
.cfd-app .cfd-title{font-size:1.36rem;margin:0;font-weight:700;letter-spacing:-.01em}
.cfd-app .cfd-sub{color:var(--cfd-muted);margin:5px 0 0;font-size:.88rem}
"#;
