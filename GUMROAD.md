# Gumroad listing

Everything needed to create/update the Gumroad product once this app is
built. Source copy is `apps.md` §T1-2 in the `tpt-electrician-nz-2` repo — if
that changes, update here too (and vice versa).

## Step-by-step: publish this listing

1. Build the Pro edition (`./build.ps1 -Pro`, once the build pipeline exists
   — see `todo.md` and mirror `tpt-app-fea-lite`/`tpt-app-cutlist-optimizer`'s
   pattern) — produces the packaged exe/zip in `release/`.
2. Log in to Gumroad → **New product** → **Digital product**.
3. Set the product name, price, and upload the packaged file (all below).
4. Paste the short description, bullets, and full description (below) into
   their respective fields.
5. Set category/tags (below) and add a cover image (see note below).
6. Publish, then copy the resulting product URL (`https://gum.co/...`).
7. Paste that URL into the `cfd-studio` entry's `gumroadUrl` field in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`, and set `price: '$349'`
   on the same entry.
8. Flip that entry's `status` to `'live'` once both the free demo and this
   Gumroad listing exist.
9. Commit and push the registry repo.

## File to upload

Not built yet. Once packaged, this will be the Pro exe/zip produced by the
app's `build.ps1 -Pro` (or equivalent) — see `todo.md`.

## Product name

```
TPT CFD Studio — Affordable Airflow Simulation for Motorsport & Drone R&D
```

## Price

**$349 USD, one-time purchase** (no subscription).

## Short description / summary field

```
Estimate drag and lift before you book wind-tunnel time. A real CFD solver at a fraction of Fluent/Star-CCM+ pricing, built for small motorsport and drone R&D teams.
```

## Bullets / feature list

```
3D external flow solver
Drag/lift coefficient output
Exportable visualizations
No annual license
One-time purchase
```

## Full description (long-form field)

```
TPT CFD Studio runs simple external-flow CFD — drag/lift estimation over a
body, basic airflow visualization — so you can iterate on shapes before
committing to a physical prototype or a wind-tunnel booking.

Who it's for: motorsport teams, drone/UAV R&D shops, and small aerodynamics
consultancies who are priced out of ANSYS Fluent or Star-CCM+ (commonly
$30,000+/year). Wind-tunnel time is expensive and slow; a rough CFD pass to
narrow down design options first is genuinely valuable at a much lower price
point.

Pro edition (this download):
- 3D external flow solve (not just 2D)
- Drag/lift coefficient output
- Exportable visualization and report

Try the free 2D qualitative edition first, right in your browser, no install:
tptsolutions.co.nz/tools/cfd-studio

Requirements: Windows 10 (20H2+) or Windows 11. Uses the WebView2 runtime,
preinstalled on both — nothing extra to download.
```

## Category / tags

- Category: **Software > Design & Engineering** (or Gumroad's closest equivalent)
- Tags: `cfd`, `aerodynamics`, `motorsport`, `drone-engineering`,
  `engineering-software`, `windows`

## Cover image / thumbnail

Not created yet. A screenshot of the 3D flow visualization around a real
sample body (car shape / airframe) is the most convincing cover once built.
No stock/AI art.

## After publishing

1. Copy the resulting product URL (`https://gum.co/...`).
2. Paste it into the `gumroadUrl` field of the `cfd-studio` entry in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`, and set `price: '$349'`.
3. Set `status` to `'live'`.
4. Commit and push the registry repo.
