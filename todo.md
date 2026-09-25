# TPT CFD Studio — Build Todo

> Hub listing: `tptsolutions.co.nz/tools/cfd-studio` (registry slug
> `cfd-studio`, category `engineering`). Spec: `apps.md` §T1-2 in the
> tpt-electrician-nz-2 repo. Free WASM edition: 2D flow visualization around a
> simple shape, qualitative only. Paid offline edition ($349 USD, Gumroad —
> apps.md and GUMROAD.md agree on $349; the $199 figure here was stale): 3D
> external flow solve, drag/lift coefficients, exportable visualization and
> report. Solver wraps `tpt-physics` (AI-native FEA/DEM/CFD engine) — the moat
> is a real CFD solver, not a toy demo. UI on `tpt-appfront`.
>
> Hub contract: `wasm-bindgen --target web` glue exposes `mount(container)`
> (called by `WasmAppRunner` after `default()` init). Standalone `trunk serve`
> mounts into the `#tpt-appfront-root` marker div instead — never
> `document.body` — so the two hosts never double-mount.

## Phase 0 — Scaffold & pipeline proof ✅
- [x] Scaffold the workspace (hand-scaffolded to the fea-lite layout rather
      than `tpt-appfront init` — same result, no CLI round-trip; path deps on
      the tpt-appfront checkout)
- [x] Hub contract: `#[wasm_bindgen] pub fn mount(el)` + `#[wasm_bindgen(start)]`
      no-op unless `#tpt-appfront-root` present
- [x] `index.html` carries the `#tpt-appfront-root` marker div
- [x] Build pipeline `build.ps1` mirroring fea-lite's (`--target web` glue →
      web repo `public/apps/cfd-studio/`); note: .ps1 needs a UTF-8 BOM +
      CRLF for Windows PowerShell 5.1
- [x] Hello-world build loads through the standalone dev page (`trunk serve`,
      verified in-browser in Phase 2 below)
- [x] git init + first commit

## Phase 1 — Solver integration (wrap tpt-physics) ✅
- [x] Survey `tpt-physics`: `tpt-phys-cfd` ships a D2Q9 LBM with
      velocity-inlet/zero-gradient-outlet x-boundaries — right base for 2D
      external flow. No 3D solver upstream (added app-side, see below).
      Upstream additions made (in tpt-physics, **uncommitted there**):
      - boundary momentum-exchange force (Ladd) + accessor,
      - Zou-Hou density-pinned outflow — the old copy-based outlet let the
        domain fill with pressure and silently decayed drag (Cd 2.1 → 1.2),
      - `add_velocity_bump` to seed the Kármán instability (a symmetric
        start holds the unsteady branch ~10⁵ steps: Cd 0.91 vs 1.33 at
        Re = 100),
      - allocation-free streaming buffer (+27% throughput).
- [x] Adapter crate `engine/`: `Problem2D` (shape, height, speed, fluid,
      resolution) → incremental `Solver2D` → velocity/pressure fields +
      momentum-exchange Cd/Cl. Plain structs, no UI deps, host-testable.
      Pro adds `threed.rs` (app-local D3Q19 — kept out of upstream on
      purpose) and `report.rs` (PDF writer).
- [x] Validation cases (release suite, `cargo test --release -p
      tpt-cfd-engine`): cylinder Cd vs published at Re 20 (1.93 vs ≈2.0-2.2),
      Re 40 (1.39 vs ≈1.5-1.65), Re 100 (1.37 vs ≈1.33-1.40) + shedding
      check; normal flat plate bracket; 4:1 streamlined vs bluff; capped
      high-Re stability; 3D sphere in band at Re 40. Honest limits: BGK
      ceiling ≈ 5·H cells Re (flagged when capped); 3D coefficients at the
      built-in resolutions are rough (shape ranking lives in 2D).
- [x] Perf budget: Quick (12 cells) animated default run = 7.5k steps on a
      144×60 lattice → 2.8 s host, ~5-6 s in-browser — it animates while it
      solves, so the wait is the show. Standard (24 cells) ≈ 4× that.
- [x] Wasm payload: **191 KB raw / 79 KB gzipped** (budget 2 MB) — engine
      crate opts into `opt-level = 3` inside the size-optimized release
      profile (`z` cost ~3x on the LBM hot loop).

## Phase 2 — Free WASM UI ✅
- [x] Body-shape picker (cylinder / square / flat plate / streamlined
      ellipse), free-stream speed + body size inputs, Quick/Standard
      resolution
- [x] 2D flow visualization: live velocity heatmap + tracer streaks + body
      outline + legend; animated stepping (rAF + timer watchdog for
      RAF-throttled contexts); qualitative only, no coefficients shown
- [x] Free-tier upsell card → tptsolutions.co.nz/tools/cfd-studio
- [x] Four one-click samples; auto-runs the vortex street on load
- [x] Browser-verified (trunk serve, dev + release): mount, auto-solve,
      rendering, live status (Re / grid / progress / steps-per-second),
      shape & quality switching, honest Re-cap note. Gotchas fixed en
      route: bare `text()` nodes can't carry classes (wrap dynamic lines in
      containers) and mount/effect handles must be `mem::forget`-pinned
      (subscribers are `Weak`).

## Phase 3 — Paid desktop edition (cargo feature `pro`) — build in flight
- [x] `pro` gates: coefficient readout card, PNG + PDF report export, 3D
      external-flow solve (D3Q19 with slice viewer)
- [x] Pro UI: 3D shape/resolution pickers, Run 3D, live 3D status +
      slice heatmaps; coefficients card populated from the solved runs
- [x] Desktop shell via `tpt-appfront-webview` (scaffolded in Phase 0)
- [x] Windows exe build + smoke test (`build.ps1 -Pro` → 1.25 MB exe,
      launches, WebView2 stack active, serves its embedded pro bundle)
- [ ] Gumroad product from apps.md listing copy ($349), exe upload, URL into
      registry — manual step, listing copy in `GUMROAD.md`

## Phase 4 — Ship & measure
- [x] Registry snippet emitted by `build.ps1` (slug `cfd-studio`, category
      `engineering`, `wasm: { entry: '/apps/cfd-studio/cfd-studio.js' }`);
      the free bundle is already copied into the web repo's
      `public/apps/cfd-studio/` — adding the registry entry + flipping
      status to `live` is a one-paste manual step in the web repo (see
      `GUMROAD.md` after the Gumroad listing exists)
- [ ] Verify hub card/detail/sitemap/runner in dev, then prod after deploy
- [x] WASM size + solve-time budget check on the shipped build (see Phase 1)
