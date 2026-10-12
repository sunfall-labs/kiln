# kiln website

Static landing page for kiln v0.1.0. This independent `gh-pages` branch contains only site files.

GitHub Pages source: **Deploy from a branch → gh-pages → / (root)**. `.nojekyll` publishes the
HTML, CSS, JavaScript and SVG directly; there is no dependency installation or custom build.
The default address is `https://sunfall-labs.github.io/kiln/`.

Preview locally with `python3 -m http.server 8765` and open `http://localhost:8765/`.
Edit `index.html`, `style.css` and `site.js`, then commit and push this branch to update the site.
Keep platform badges, archive links and status aligned with the published release. Do not
advertise an unreleased binary as available.

GitHub Free supports Pages from public repositories. A private organization repository needs a
paid plan. Repository visibility must only change with the owner's explicit approval.
