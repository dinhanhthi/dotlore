# Landing page

Static HTML for [Dotlore](https://dotlore.dinhanhthi.com). No build step. No tests.

A push to `main` that touches this folder deploys to GitHub Pages via [`.github/workflows/pages.yml`](../.github/workflows/pages.yml) — but only once the version badge names the latest published release. A release commit's badge bump is held until `scripts/release-local.sh` publishes the release; the script then dispatches the deploy. Live site: <https://dotlore.dinhanhthi.com>.

## Preview

From the repo root:

```sh
pnpm website:dev   # http://localhost:38423
```

Or open `website/index.html` in a browser.
