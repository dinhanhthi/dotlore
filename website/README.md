# Landing page

Static HTML for [Dotlore](https://dotlore.dinhanhthi.com). No build step. No tests.

A push to `main` that touches this folder deploys to GitHub Pages via [`.github/workflows/pages.yml`](../.github/workflows/pages.yml). Live site: <https://dotlore.dinhanhthi.com>.

## Preview

From the repo root:

```sh
open website/index.html
```

Or serve the folder with any static server.

## Files

| File | Role |
| --- | --- |
| `index.html` | Page |
| `styles.css` | shadcn-like tokens and layout |
| `logo.png` | Copy of `../assets/logo_256.png` |
| `CNAME` | Custom domain (`dotlore.dinhanhthi.com`) |
| `.nojekyll` | Skip Jekyll on GitHub Pages |

Do not couple this folder to `src/`.
