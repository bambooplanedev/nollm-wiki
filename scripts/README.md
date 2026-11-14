# scripts/ — Quartz Publishing Workflow

This directory contains the tooling to turn a compiled wiki directory into a local Quartz static site for preview and deployment.

## What It Does

`quartz-publish.sh` takes a compiled wiki directory (from `kalka`) and publishes it as a [Quartz](https://quartz.jzhao.xyz/) static site. The script:
1. Clones Quartz (pinned to `v4.5.2`) into `.quartz-build/` (reused across runs)
2. Runs `npm install` once (then cached; retried if it did not finish)
3. Copies wiki pages through `strip-chrome.awk` to remove compiler chrome, leaving out agent memory notes
4. Builds the static site or serves a live preview locally

## Prerequisites

- **Node.js** ≥ 22, `git` and `perl`
- **Network access** for the one-time initial clone of the Quartz repository and `npm install`
- A compiled wiki directory with `index.md` and `index.json`

## Usage

### Static Build

Build a static site (output in `.quartz-build/public/`):

```bash
scripts/quartz-publish.sh /path/to/project-wiki --title "My Project"
```

The built site will be in `.quartz-build/public/`. Open `public/index.html` in a browser to view it.

### Live Local Preview

Serve a live preview with hot-reloading at `http://localhost:8080`:

```bash
scripts/quartz-publish.sh /path/to/project-wiki --serve
```

Press Ctrl-C to stop the server.

### Options

- `<compiled-wiki-dir>` (required): path to the compiled wiki directory (must contain `index.md` and `index.json`)
- `--title "Site Title"`: set the site title in `quartz.config.ts` (defaults to the directory basename)
- `--serve`: serve a live preview instead of a static build

## What Is Published vs. Excluded

**Published:**
- `index.md` — the landing page
- Every `<id>.md` page — individual wiki pages

**Excluded:**
- `AGENTS.md` — agent navigation notes are not published
- Agent memory notes — pages flagged `"memory": true` in `index.json`, and the compiler's link lines to
  them (`- [[id|...]]` list items and their `index.md` entry; links in page bodies are kept). `kalka serve`,
  `kalka setup` and `kalka compile --memory DIR` make such pages, and so does a plain `kalka compile` when the
  source has `.agents/memory/`. The page count and link degrees in `index.md` still include them.
- Non-markdown files: `index.json`, `items.json`, `llms.txt`, `.wiki/`

## Content Transformation

The `strip-chrome.awk` script removes compiler chrome before publishing:

- **`## Imported by`** and **`## Mentioned in`** — the compiler's backlink sections; Quartz draws its own backlinks
- **`## Notes`** — the compiler's trailing section (the last `## Notes` to the end of the page), with the
  placeholder or hand-written notes alike; hand notes are not published

Only headings above a page's `## Body` are taken as chrome, so a markdown body that uses the same
headings keeps them.

### Testing

`strip-chrome.awk` has a self-contained, network-free unit test:

```bash
scripts/test-strip-chrome.sh
```

It compiles a small source tree with the current compiler (`$WIKI_BIN` if set, else `cargo run`), runs
the awk filter over the pages and asserts that the chrome and hand-written notes are removed while body content
stays. CI runs it after `cargo test`.

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

`selfhost.sh` — compiles this repository into its own wiki at `.kulman/`
(always rebuilds the release binary first) and fails on a broken wikilink
(`kalka lint`). See the repo root
[`README.md`](../README.md#self-hosting).

Search-quality measurement now lives in `crates/kalka/tests/eval.rs`, which scores a
frozen corpus rather than the moving self-hosted one and runs in CI
(`.github/workflows/rust.yml`: fmt, clippy, test, the strip-chrome test, docs). See the
Testing section of [`crates/kalka/docs/ARCHITECTURE.md`](../crates/kalka/docs/ARCHITECTURE.md).
