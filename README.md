# kiln website

Static feature and measurement site for kiln v0.1.0. This independent `gh-pages` branch contains only site files.

GitHub Pages source: **Deploy from a branch → gh-pages → / (root)**. `.nojekyll` publishes the
HTML, CSS, JavaScript and data directly; there is no dependency installation or deployment build.
The default address is `https://sunfall-labs.github.io/kiln/`.

Preview locally with `python3 -m http.server 8765` and open `http://localhost:8765/`.
Edit `index.html`, `style.css` and `site.js`, then commit and push this branch to update the site.
The quantitative source of truth is `data/benchmarks.json`, transcribed from the v0.1.0
README measurement record. It includes the machine, date, runtime versions and workload/client
conditions. Do not invent speedups for parallel collections or actors: their diagrams describe
execution and ownership, not measured performance.

After editing measurement data, run `python3 scripts/render-benchmarks.py`. This refreshes the
initial bars, full accessible tables and the embedded dataset used by chart controls. Check its
output is unchanged on a second run. Keep chart axes and headings aligned if changing the data.
The page includes complete, downloadable programs under `examples/`; keep visible excerpts
consistent with them and check both execution modes when changing those examples.

Keep platform badges, archive links and status aligned with the published release. Do not
advertise an unreleased binary as available.

GitHub Free supports Pages from public repositories. A private organization repository needs a
paid plan. Repository visibility must only change with the owner's explicit approval.

Charts and explanatory diagrams are SVG. The benchmark renderer also writes standalone charts
under `assets/charts/`, offered as downloads on the page. Interactive charts update SVG bar widths
and accessible descriptions from the same JSON data; ordinary HTML tables retain every measurement.

The homepage leads with TypeScript, including `kiln check`, typed actor examples and a conformance
section. `reference/` contains checker architecture, conformance evidence and concurrency/ownership
pages. Counts come from a pinned release snapshot under `data/conformance.json`; preserve the
source commit, corpus pins and distinctions between exact diagnostics, partial matches, runtime
variants and package compatibility.

To refresh for a deliberately selected source revision:

```sh
python3 scripts/import-conformance.py --repo ../kiln --ref v0.1.0
python3 scripts/render-conformance.py
```

The renderer refreshes chart SVGs, homepage counts, accessible tables and reference pages. It is
idempotent. Reference page prose is maintained in the renderer, while downloadable programs live
under `examples/`; changed TypeScript samples must pass `kiln check` and both execution modes.
TS7 means pinned TypeScript 7.0.2 libraries and reference comparisons, not full drop-in compatibility.

`data/npm-compat.json` records all 361 package names, exact versions and smoke outcomes from the
same source revision. The importer verifies this inventory against the pinned npm manifest.
The npm reference page offers a progressive search; without JavaScript the full table remains
available. Keep its native-dev profile, Node reference version and production-profile caveat visible.
`reference/builtins.html` lists public modules and global extensions, with process, server and JSX
samples. Linux release archives have no GUI provider; type checking the GUI sample is portable,
but interactive execution must be checked with the Apple Silicon macOS release.

Comparison prose is maintained in `scripts/render-comparisons.py`. Keep competitor claims linked
to primary documentation and dated; recheck them before updating the review date. The pages
compare workflows and documented capabilities, not matched performance. The existing Hermes
benchmark lane runs HBC through hvm and cannot support Static Hermes native-AOT rankings.

After editing or regenerating any HTML, run:

```sh
python3 scripts/render-conformance.py
python3 scripts/render-comparisons.py
python3 scripts/render-runtime-results.py
python3 scripts/render-seo.py
```

The last command adds unique canonical URLs, Open Graph/Twitter metadata, WebPage and breadcrumb
structured data, and `sitemap.xml`. Use the public GitHub Pages base URL until the site actually
moves to another domain; a project-level robots.txt cannot control the host's root robots policy.
Verify all four renderers together are idempotent, and review comparisons at mobile and desktop
widths. No search rankings or rich-result eligibility are implied by the metadata.

The shared comparison suite and frozen profiles are described in `benchmarks/README.md`.
`data/runtime-comparison-2026-10-12.json` retains all selected outcomes and raw timing samples;
`render-runtime-results.py` turns it into charts, a compatibility matrix and pairwise recommendations.
Never count a compile failure as a passing capability or display a time for wrong output. The
24-probe denominator is the selected shared suite, not Test262 or the 361-package corpus.

The repository microbenchmark explorer uses `data/microbenchmarks.json`, with the exact original
October 4 record retained in `data/microbench-2026-10-04.json`. The renderer asserts that all 22
rows match the original record and that it contains no divergences. Do not silently replace one
row with a later experiment: show follow-up results separately until a complete suite is measured.
The source commit and original compiler digest stay visible or downloadable. These are JavaScript
workloads, separate from the released native-runtime comparison and the HTTP measurement.
