# scripts/ — Quartz Publishing Workflow

This directory contains the tooling to turn a compiled wiki directory into a local Quartz static site for preview and deployment.

## What It Does

`quartz-publish.sh` takes a compiled wiki directory (from the `wiki` compiler) and publishes it as a [Quartz](https://quartz.jzhao.xyz/) static site. The script:
1. Clones Quartz (pinned to `v4.5.2`) into `.quartz-build/` (reused across runs)
2. Runs `npm install` once (then cached)
3. Copies wiki pages through `strip-chrome.awk` to remove compiler-owned metadata sections
4. Builds the static site or serves a live preview locally

## Prerequisites

- **Node.js** ≥ 22
- **Network access** for the one-time initial clone of the Quartz repository and `npm install`
- A compiled wiki directory with an `index.md` file

## Usage

### Static Build

Build a static site (output in `.quartz-build/public/`):

```bash
scripts/quartz-publish.sh /path/to/rss-feed-wiki --title "RSS Feed"
```

The built site will be in `.quartz-build/public/`. Open `public/index.html` in a browser to view it.

### Live Local Preview

Serve a live preview with hot-reloading at `http://localhost:8080`:

```bash
scripts/quartz-publish.sh /path/to/rss-feed-wiki --serve
```

Press Ctrl-C to stop the server.

### Options

- `<compiled-wiki-dir>` (required): path to the compiled wiki directory (must contain `index.md`)
- `--title "Site Title"`: set the site title in `quartz.config.ts` (defaults to the directory basename)
- `--serve`: serve a live preview instead of a static build

## What Is Published vs. Excluded

**Published:**
- `index.md` — the landing page
- Every `<id>.md` page — individual wiki pages

**Excluded:**
- `AGENTS.md` — agent definitions are not published
- Non-markdown files: `index.json`, `graph.json`, `llms.txt`, `.wiki/`

## Content Transformation

The `strip-chrome.awk` script removes compiler-owned metadata sections before publishing:

- **`## Referenced By`** — Quartz provides native backlink functionality; this compiler-added section is redundant
- **`## Notes`** — compiler-added placeholder; removed to keep pages clean

### Known Limitation

An orphan page (with no incoming links) whose markdown body contains a literal `## Referenced By` heading as actual content would have that body heading stripped. This is rare and cosmetic — the orphan page's content would still be readable, but the heading would be missing.

### Known Issues

Two argument-parsing edges in `quartz-publish.sh`, found in review and
left as documented follow-ups. Both fail loud (a build error or a
visibly wrong mode), never silent corruption:

- A `--title` value ending in a backslash can break the generated
  string in `quartz.config.ts` — the title sanitizer replaces `"` with
  `'` but leaves `\` alone.
- `--title --serve` consumes `--serve` as the title value; there is no
  guard against flag-shaped values following `--title`.

### Testing

`strip-chrome.awk` has a self-contained, network-free unit test:

```bash
scripts/test-strip-chrome.sh
```

It runs the awk filter over fixture pages and asserts that compiler-owned sections are removed while body content is preserved.

## Quartz Version

The script uses a pinned Quartz version: **`v4.5.2`**

The Quartz repository is cloned into `.quartz-build/` on first run and cached across runs (added to `.gitignore`). To force a fresh clone or change the Quartz version, delete the `.quartz-build/` directory.

## Deploying to a Static Host

The built site is in `.quartz-build/public/`. To deploy:

1. Edit `.quartz-build/quartz.config.ts` and set the `baseUrl` if your site is hosted on a non-root path
2. Push the `public/` directory to any static host:
   - GitHub Pages
   - Netlify
   - Cloudflare Pages
   - Any other static file hosting

For detailed hosting setup, see [Quartz: Hosting](https://quartz.jzhao.xyz/hosting).

## Also in this directory

`selfhost.sh` — compiles this repository into its own wiki at `.wiki/`
(always rebuilds the release binary first). See the repo root
[`README.md`](../README.md#self-hosting).

Search-quality measurement now lives in `wiki/tests/eval.rs`, which scores a
frozen corpus rather than the moving self-hosted one and runs in CI
(`.github/workflows/rust.yml`: fmt, clippy, build, test). See the
Testing section of [`wiki/docs/ARCHITECTURE.md`](../wiki/docs/ARCHITECTURE.md).
