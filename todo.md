# TPT CFD Studio — Build Todo

> Hub listing: `tptsolutions.co.nz/tools/cfd-studio` (registry slug
> `cfd-studio`, category `engineering`). Spec: `apps.md` §T1-2 in the
> tpt-electrician-nz-2 repo. Free WASM edition: 2D flow visualization around a
> simple shape, qualitative only. Paid offline edition ($199 USD, Gumroad): 3D
> external flow solve, drag/lift coefficients, exportable visualization and
> report. Solver wraps `tpt-physics` (AI-native FEA/DEM/CFD engine) — the moat
> is a real CFD solver, not a toy demo. UI on `tpt-appfront`.
>
> Hub contract: `wasm-bindgen --target web` glue exposes `mount(container)`
> (called by `WasmAppRunner` after `default()` init). Standalone `trunk serve`
> mounts into the `#tpt-appfront-root` marker div instead — never
> `document.body` — so the two hosts never double-mount.

## Phase 0 — Scaffold & pipeline proof
- [ ] `tpt-appfront init tpt-app-cfd-studio --target dom` (path deps on the tpt-appfront checkout)
- [ ] Restructure scaffold to hub contract: `#[wasm_bindgen] pub fn mount(el)` + `#[wasm_bindgen(start)]` no-op unless `#tpt-appfront-root` present
- [ ] `index.html` carries the `#tpt-appfront-root` marker div
- [ ] Build pipeline script mirroring fea-lite/cutlist-optimizer's (`--target web` glue → web repo `public/apps/cfd-studio/`)
- [ ] Hello-world build loads through the standalone dev page (`trunk serve`)
- [ ] git init + first commit

## Phase 1 — Solver integration (wrap tpt-physics)
- [ ] Survey `tpt-physics` crates for the CFD-relevant solver path (likely a
      finite-volume or lattice-Boltzmann external-flow module) — confirm what
      exists vs. what needs to be added upstream
- [ ] Adapter crate `engine/`: problem in (body geometry, inlet velocity,
      fluid properties) → results out (velocity/pressure field, drag/lift
      coefficients) as plain structs — no UI deps, host-unit-testable
- [ ] Validation cases with known references (flat plate drag, cylinder in
      crossflow Cd vs. Reynolds number) to establish accuracy bounds
- [ ] Perf budget: in-browser solve time for the free 2D case and the pro 3D
      case (target: sub-few-second for free tier, define pro-tier ceiling)
- [ ] Wasm payload size check once the sub-crate set is chosen

## Phase 2 — Free WASM UI
- [ ] Simple body-shape picker (parametric 2D profile) + inlet velocity input
- [ ] 2D flow visualization (streamlines or velocity-field color overlay),
      qualitative only, no coefficient output
- [ ] Free-tier upsell copy pointing at the paid 3D/coefficient/export features
- [ ] Sample shapes for instant first-run value
- [ ] Browser-verified: mount, solve, visualization renders correctly

## Phase 3 — Paid desktop edition (cargo feature `pro`)
- [ ] `pro` gates: 3D external flow solve, drag/lift coefficient output,
      exportable visualization (image/video) and PDF-style report
- [ ] Pro UI: 3D body/mesh setup, coefficient results table, export buttons
- [ ] Desktop shell via `tpt-appfront-webview`; Windows exe build + smoke test
- [ ] Gumroad product from apps.md listing copy ($199), exe upload, URL into
      registry — manual step, listing copy at apps.md §T1-2

## Phase 4 — Ship & measure
- [ ] Registry entry (`engineering` category, `wasm: { entry: '/apps/cfd-studio/cfd-studio.js' }`)
      per the "Adding a finished app to the hub" runbook at the bottom of apps.md
- [ ] Verify hub card/detail/sitemap/runner in dev, then prod after deploy
- [ ] WASM size + solve-time budget check on the shipped build
