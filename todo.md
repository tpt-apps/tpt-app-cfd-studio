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
      registry — manual step, listing copy in `GUMROAD.md`. **Blocked on
      Phase 3b**: as built, the Pro edition has five preset 3D shapes, no
      geometry import, and a laminar 3D solver capped near Re 60 whose
      coefficients the app itself calls "rough estimates" — not worth $349 to
      the motorsport/drone audience in the listing.

## Phase 3b — PARKED (3D research track; 2D ships)
Decision: the 3D solver is not sellable as-is. Findings:
- LES validation at 12 cells, Re 10⁵, 6 000 steps: cube Cd 1.06 (ref ≈ 1.05),
  sphere 0.76 (ref ≈ 0.47), Ahmed 0.93 (ref 0.285, with ground — not
  comparable), plate 1.11, 4:1 ellipsoid 0.80 (should be far below the sphere).
  Shape ranking fails for the streamlined body.
- Root cause: the 3D solver does not simulate the requested Reynolds number.
  The molecular τ floor (0.502) gives a molecular Re ≈ 900 at 12 cells, and the
  Smagorinsky term can only add viscosity. Re 10³ and Re 10⁵ produce identical
  results. Laminar BGK holds only for Re ≲ 30 at 12 cells.
- Reynolds matching needs a wall-modelled LES calibrated against published data
  and validated across Re, on a finer grid. Estimated: weeks plus substantial
  compute, no guarantee. Not started.
- The 3D code remains in the repo behind the `pro` feature (not wired to any
  product claim). Shelved until a customer needs Reynolds-matched 3D data.

Shipping: 2D solver (validated cylinder Re 20/40/100 and streamlined ranking),
free edition plus a 2D Pro tier (coefficient readout, PNG and PDF export).
Price: $49 one-time (decided). See `GUMROAD.md`.

Archived task list (for reference only, not active):
Why: real car/drone flows are turbulent (Re ~10⁵–10⁷) and users need to run
their own parts. Neither works today.

Done so far:
- [x] Fix 3D picker highlight bug (class attribute got the selector's leading
      dot, so shape/resolution selection never showed) — `src/app.rs`
- [x] Pro header chip no longer says "free · 2D qualitative"
- [x] Dark mode: page `html`/`body` background + `color-scheme` (white strip)
- [x] Ahmed body (road-car bluff-body reference: 3.6H × 1.35H × H, rounded
      nose, 25° slant) replaces the box-and-cabin placeholder

To do:
- [x] **Turbulence model**: Smagorinsky LES in the D3Q19 collision step
      (non-equilibrium momentum flux → effective τ), switched on when the
      laminar BGK would fall below `TAU_MIN`. Removes the Re ≈ 5·H-cells
      ceiling (≈ Re 60 at 12 cells). Report "turbulent (LES)" instead of
      "capped"; keep laminar BGK below the stable band so the 2D-validated
      low-Re behaviour doesn't change. Needs a τ floor and a stability check
      at Re 10⁵–10⁶.
- [x] **STL import** (engine): binary + ASCII STL parser; voxelise the mesh into
      the lattice (ray parity along the flow axis; scale so the up-axis extent
      = body height); size the domain from the mesh extents (nx/ny/nz);
      frontal area from the mask (already done); cell-count cap with a clear
      error; unit tests (cube, icosphere volume vs analytic, binary/ASCII
      round-trip, non-watertight handling).
- [x] **STL import** (UI): file picker (`<input type=file accept=.stl>` →
      `FileReader` → bytes → engine), loaded-body label with triangle count,
      "reverse flow" and "up axis Z/Y" toggles, picking a preset clears the
      mesh. Needs extra `web-sys` features (`File`, `FileReader`, …). Verify
      the picker works inside the WebView2 shell.
- [x] **Validation** (release suite, ignored/slow where needed). Measured at
      Re 10⁵ / 12 cells across the body, 6 000 steps, LES on (`les3d.rs`):
      - cube (face-on): Cd **1.055** vs ≈ 1.05 published — match
      - sphere: Cd **0.762** vs ≈ 0.47 published — overpredicts ~60%
      - Ahmed 25° (Re 10⁶, 8 000 steps): Cd **0.928** vs 0.285 published (with
        ground plane; model is free-stream) — overpredicts ~3×
      - imported STL sphere Cd 0.799 vs preset 0.762 (within the 25% bound;
        measured 5%)
      - 2D cylinder suite (Re 20/40/100, flat plate, streamlined, capped Re):
        6/6 pass
      Not tuned to pass. The 3D gap is the coarse 12-cell lattice; the claim
      is shape ranking, with absolute coefficients labelled indicative.
- [x] Update `GUMROAD.md` copy to match what the solver actually delivers
      (STL import, LES, measured validation; wind-tunnel framing dropped)
- [ ] Re-take the cover/screenshots after the above (imported real car body,
      ≥ 1080p, status caveats cropped or resolved); no stock/AI art
- [x] Re-run `build.ps1 -Pro` (builds clean with `NO_COLOR=1` set; exe produced)
- [ ] Smoke-test the exe by hand (picker, run, export PNG/PDF) — GUI check, not run here

Housekeeping found along the way:
- [x] `dist/` is git-tracked but is a build artefact (hashed filenames churn on
      every build) — untracked and `.gitignore`d
- [x] `build.ps1` fails when `NO_COLOR=1` is set (trunk 0.21 wants
      `true`/`false`) — unset it inside the script for the trunk call
- [ ] Windows Firewall prompts for the app's webview may block input until
      answered — check whether the shipped exe triggers one and document it
- [ ] Commit the uncommitted changes in `src/app.rs`, `engine/src/threed.rs`

## Phase 4 — Ship & measure
- [x] Registry snippet emitted by `build.ps1` (slug `cfd-studio`, category
      `engineering`, `wasm: { entry: '/apps/cfd-studio/cfd-studio.js' }`);
      the free bundle is already copied into the web repo's
      `public/apps/cfd-studio/` — adding the registry entry + flipping
      status to `live` is a one-paste manual step in the web repo (see
      `GUMROAD.md` after the Gumroad listing exists)
- [ ] Verify hub card/detail/sitemap/runner in dev, then prod after deploy
- [x] WASM size + solve-time budget check on the shipped build (see Phase 1)
