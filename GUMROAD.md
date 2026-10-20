# Gumroad listing (on hold)

Status: **ready once the exe is smoke-tested.** The 3D solver is parked (see
`todo.md`, Phase 3b). The listing below is scoped to the 2D solver, which is
the validated product. Price is $49 (replaces the earlier $349, which was built
on 3D claims that no longer stand).

## Step-by-step: publish this listing (once price and build are settled)

1. Build the Pro edition (`./build.ps1 -Pro`) — produces
   `target\release\tpt-cfd-studio-pro.exe`.
2. Smoke-test the exe: 2D run, coefficient readout, export PNG and PDF.
3. Log in to Gumroad → **New product** → **Digital product**.
4. Set the product name, price, and upload the packaged file.
5. Paste the short description, bullets, and full description (below).
6. Set category/tags and add a cover image.
7. Publish, then copy the product URL (`https://gum.co/...`).
8. Paste that URL into the `cfd-studio` entry's `gumroadUrl` in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`, set the price on the
   same entry, and flip `status` to `'live'` once the free demo and this
   listing both exist.

## Product name

```
TPT CFD Studio — 2D Airflow Shape Comparison
```

## Price

**$49 USD, one-time purchase** (no subscription).

## Short description / summary field

```
See how airflow moves around a body and compare drag between simple shapes, live in your browser or on your desktop. Validated against published 2D data.
```

## Bullets / feature list

```
2D external-flow solver (D2Q9 lattice-Boltzmann)
Drag and lift coefficient readout, validated against published cylinder data
Flow visualization with live velocity and tracer streaks
Export flow images (PNG) and a PDF report
One-time purchase, no annual license
```

## Full description (long-form field)

```
TPT CFD Studio simulates airflow around a 2D body — a cylinder, square, flat
plate or streamlined ellipse — and shows the flow and its drag and lift
coefficients. Use it to see how shape changes airflow before you build a
prototype.

What it is good for: comparing 2D shape options and building intuition for
where flow separates and drag comes from. It is a 2D model, not a 3D car or
drone simulation.

Validation (measured, reported as-is):
- Cylinder drag at Re 20, 40 and 100 falls within published ranges
- The streamlined (4:1) ellipse has clearly lower drag than the cylinder at
  the same Reynolds number, as expected

Try the free 2D qualitative edition first, right in your browser, no install:
tptsolutions.co.nz/tools/cfd-studio

Requirements: Windows 10 (20H2+) or Windows 11. Uses the WebView2 runtime,
preinstalled on both — nothing extra to download.
```

## Category / tags

- Category: **Software > Design & Engineering** (or Gumroad's closest equivalent)
- Tags: `cfd`, `aerodynamics`, `engineering-software`, `windows`

## Cover image / thumbnail

Not created yet. A flow visualization around a cylinder or ellipse, at ≥ 1080p.
No stock/AI art.
