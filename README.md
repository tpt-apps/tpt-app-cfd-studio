# TPT CFD Studio

Real lattice-Boltzmann CFD in the browser (free, qualitative 2D) and on the
desktop (Pro: drag/lift coefficients, 3D external flow, exports). Part of the
[TPT Solutions app hub](https://tptsolutions.co.nz/tools/cfd-studio) — see
`todo.md` (build state) and `GUMROAD.md` (listing copy).

## Layout

```
src/        DOM app (tpt-appfront-dom) — hub-contract mount + standalone
engine/     tpt-cfd-engine: UI-free solver adapter, host-testable
desktop/    Pro WebView2 shell (embeds the trunk bundle into the exe)
build.ps1   free bundle (default) / Pro bundle + exe (-Pro)
```

- `engine/` wraps [`tpt-phys-cfd`](https://github.com/tpt-solutions/tpt-physics)'s
  D2Q9 lattice-Boltzmann solver: Reynolds-matched lattice setup, incremental
  stepping (so the browser can animate the flow developing), boundary
  momentum-exchange drag/lift, honest Reynolds capping at the BGK stability
  ceiling, and wake-instability seeding. The `pro` feature adds a D3Q19 3D
  solver and a dependency-free PDF report writer.
- Upstream additions in tpt-physics this app relies on: momentum-exchange
  force accessor, Zou-Hou density-pinned outflow, Kármán seeding bump,
  allocation-free streaming.

## Develop

```
trunk serve                      # standalone dev (mounts into #tpt-appfront-root)
cargo test -p tpt-cfd-engine     # fast unit tests
cargo test --release -p tpt-cfd-engine          # validation suite (cylinder Cd vs Re, plate, ...)
cargo test --release -p tpt-cfd-engine -- --ignored   # perf probe
```

## Build

```
./build.ps1        # free WASM bundle -> web repo public/apps/cfd-studio/ (or dist\web)
./build.ps1 -Pro   # pro trunk bundle (dist/) + desktop exe (target\release\tpt-cfd-studio-pro.exe)
```

The free bundle must never be built with `--features pro` — the two editions
are separate builds writing to separate directories on purpose (see
`build.ps1` header). Pro desktop resolves its bundle from `TPT_CFD_DIST`,
`./dist`, `dist` next to the exe, or its embedded copy, in that order.

## Accuracy & honest limits

- Validated: cylinder drag vs published values at Re = 20/40/100 (±10-15%),
  vortex shedding, flat-plate scale. `engine/tests/validation.rs` pins these.
- Reynolds above ≈ 5×(cells across the body) exceeds the BGK stability
  ceiling: the solver runs at the ceiling and flags it — never silently.
- 3D coefficients at the built-in resolutions are rough estimates; 2D carries
  shape-to-shape comparisons. Not a substitute for engineering analysis.
