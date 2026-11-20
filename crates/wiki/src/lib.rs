//! The `wiki` compiler library: `compile` turns a source tree into a deterministic markdown wiki plus index files.

pub mod cache;
pub mod calls;
pub mod dirlock;
pub mod formats;
pub mod generator;
pub mod graph;
pub mod hash;
pub mod hint;
pub(crate) mod ids;
pub mod labels;
pub mod lint;
pub(crate) mod live;
pub mod manifest;
pub use techlog as memory;
pub mod model;
pub mod outline;
pub mod query;
pub mod rewrite;
pub mod serve;
pub mod setup;
pub mod walk;
pub mod watch;

use crate::formats::Registry;
use crate::ids::{disambiguate_ids, disambiguate_taken, remap_reserved_names};
use crate::model::{slugify, Entity, LintReport, Origin};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
pub(crate) use wiki_code::support::is_under;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WikiError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("rayon pool build failed: {0}")]
    Pool(String),
    #[error("watch failed: {0}")]
    Watch(String),
    #[error("serve failed: {0}")]
    Serve(String),
    #[error("{0}")]
    Source(String),
}

/// Fails, naming the path and the reason, unless `source` is a folder: a typo in the source path
/// must not give an empty wiki. Checked before anything is created.
pub fn check_source(source: &Path) -> Result<(), WikiError> {
    let named = |e: &dyn std::fmt::Display| WikiError::Source(format!("{}: {e}", source.display()));
    if std::fs::metadata(source).map_err(|e| named(&e))?.is_dir() {
        Ok(())
    } else {
        Err(named(&"not a directory"))
    }
}

#[allow(clippy::struct_excessive_bools)] // independent switches, not states
pub struct CompileOptions {
    pub incremental: bool,
    pub respect_ignore: bool,
    pub emit_json: bool,
    pub jobs: Option<usize>,
    pub project: Option<String>,
    /// Extra agent memory folders (absolute), compiled as flagged pages;
    /// a missing one is skipped but still watched.
    pub memory: Vec<PathBuf>,
    /// Compile the repo's `.agents/memory`. Off, the folder is neither
    /// compiled (as memory or as plain pages) nor watched.
    pub repo_memory: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            incremental: false,
            respect_ignore: true,
            emit_json: false,
            jobs: None,
            project: None,
            memory: Vec::new(),
            repo_memory: true,
        }
    }
}

pub struct CompileResult {
    pub pages_written: usize,
    pub pages_total: usize,
    pub lint: LintReport,
    /// What this compile's walk visited; see [`watch::WatchSet`].
    pub watch_set: watch::WatchSet,
}

/// Compile `input` into a deterministic wiki under `output`.
///
/// Output is byte-identical regardless of `opts.jobs` (thread count) — all
/// parallel stages collect into ordered `Vec`s or `BTreeMap`s before anything
/// touches disk, so scheduling order never leaks into the result.
pub fn compile(
    input: &Path,
    output: &Path,
    opts: &CompileOptions,
) -> Result<CompileResult, WikiError> {
    check_source(input)?;
    let run = || compile_inner(input, output, opts);
    match opts.jobs {
        Some(n) => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .map_err(|e| WikiError::Pool(e.to_string()))?;
            pool.install(run)
        }
        None => run(),
    }
}

/// Delete every page the compiler previously wrote that is no longer live.
///
/// The prunable set is the union of the loaded cache and `prior_page_ids`,
/// which deliberately ignores the cache's version guard — without it, a
/// `tool_version` bump resets the cache and orphans every page the previous
/// version wrote, with no mechanism left to remove them.
///
/// Safe by construction: both sources list only ids the compiler itself
/// recorded, so a file the user put here is never a candidate.
///
/// A page whose notes were migrated onto a renamed page is in `consumed` and
/// is deleted. A page carrying hand-written notes that were NOT migrated is
/// kept and reported: this pass and the rename that orphaned it ship in the
/// same run, so a deletion here would be unrecoverable and silent.
fn prune_stale(
    prunable: &BTreeSet<String>,
    live: &BTreeSet<String>,
    output: &Path,
    consumed: &BTreeSet<PathBuf>,
) {
    for stale in prunable.iter().filter(|k| !live.contains(*k)) {
        let path = output.join(format!("{stale}.md"));
        if !consumed.contains(&path) && !rewrite::read_notes_from(&path).is_empty() {
            eprintln!(
                "warning: keeping {} — it carries hand-written notes that were not migrated",
                path.display()
            );
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("warning: could not remove stale page {stale}.md: {e}");
            }
        }
    }
}

fn compile_inner(
    input: &Path,
    output: &Path,
    opts: &CompileOptions,
) -> Result<CompileResult, WikiError> {
    // Compile every language query up front. They are constant strings, so a
    // failure is a programming error — but it must surface here, before any
    // page is written, rather than lazily inside a rayon worker with output
    // already committed for other files.
    formats::code::validate_queries();
    let timings = std::env::var_os("WIKI_TIMINGS").is_some_and(|v| v == "1");
    let t0 = Instant::now();

    let project = opts.project.clone().unwrap_or_else(|| {
        input
            .file_name()
            .map_or_else(|| "wiki".into(), |s| s.to_string_lossy().to_string())
    });

    // 1. Walk + parallel extract (ordered — collecting into a Vec preserves
    // the sorted-by-rel_path order that `walk` produced). Files under
    // `output` are excluded so a nested output dir never feeds its own
    // generated pages back in as sources.
    let registry = Registry::with_defaults();
    let walked = walk::walk(input, opts.respect_ignore, &|p| registry.handles(p));

    // Memory folders: the repo's `.agents/memory` and the extra folders.
    // All are walked with every ignore rule off (they are usually
    // git-excluded on purpose); markdown and text only, never `logs/`. The
    // main walk may reach the in-tree one (`--no-ignore`), so its files are
    // dropped there and come back only from the memory walk, flagged (or
    // not at all with `repo_memory` off). One folder given twice is walked once.
    let in_tree = input.join(memory::REPO_DIR);
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    if opts.repo_memory && in_tree.is_dir() {
        roots.push((in_tree.clone(), true));
    }
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    for extra in opts.memory.iter().filter(|d| d.is_dir()) {
        if !roots.iter().any(|(r, _)| same(r, extra)) {
            roots.push((extra.clone(), false));
        }
    }
    let is_note = |p: &str| {
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["md", "markdown", "txt"].contains(&e.to_ascii_lowercase().as_str()))
    };
    let files: Vec<_> = walked
        .files
        .into_iter()
        .filter(|sf| !is_under(&sf.abs_path, output))
        .filter(|sf| !is_under(&sf.abs_path, &in_tree))
        .filter(|sf| !roots.iter().any(|(r, _)| is_under(&sf.abs_path, r)))
        .collect();
    let mut dirs: Vec<PathBuf> = walked
        .dirs
        .into_iter()
        .filter(|d| !is_under(d, output) && !is_under(d, &in_tree))
        .collect();
    let mut memory_files = Vec::new();
    let mut virtual_paths: BTreeMap<String, String> = BTreeMap::new();
    // Origins of the repo folder's notes, by their final `source_path`.
    let mut origins: BTreeMap<String, model::Origin> = BTreeMap::new();
    for (root, in_repo) in &roots {
        let mut repo = if *in_repo {
            memory::repo_origins(root)
        } else {
            BTreeMap::new()
        };
        let w = walk::walk_memory(root, &|p| {
            registry.handles(p) && is_note(p) && !p.starts_with("logs/")
        });
        dirs.extend(w.dirs.into_iter().filter(|d| !is_under(d, output)));
        let abs_root = std::path::absolute(root)?;
        for mut sf in w
            .files
            .into_iter()
            .filter(|sf| !is_under(&sf.abs_path, output))
        {
            sf.rel_path = if *in_repo {
                let rel = format!("{}/{}", memory::REPO_DIR, sf.rel_path);
                if let Some(o) = repo.remove(&sf.rel_path) {
                    origins.insert(rel.clone(), o);
                }
                rel
            } else {
                let abs = memory::display_path(&abs_root.join(&sf.rel_path));
                virtual_paths.insert(abs.clone(), format!("memory/{}", sf.rel_path));
                abs
            };
            memory_files.push(sf);
        }
    }
    let watch_set = watch::WatchSet::new(
        input,
        files
            .iter()
            .chain(&memory_files)
            .map(|sf| sf.abs_path.clone()),
        dirs,
    )
    .with_memory(
        opts.repo_memory
            .then_some(in_tree)
            .into_iter()
            .chain(opts.memory.iter().cloned()),
    );
    let t_walk = Instant::now();
    let mut extracted: Vec<Entity> = files
        .par_iter()
        .filter_map(|sf| registry.extract(&sf.rel_path, &sf.bytes))
        .collect();
    let notes: Vec<Entity> = memory_files
        .par_iter()
        .filter_map(|sf| registry.extract(&sf.rel_path, &sf.bytes))
        .map(|mut e| {
            e.memory = true;
            e.origin = origins.get(&e.source_path).cloned().unwrap_or_default();
            e
        })
        .collect();
    let t_extract = Instant::now();

    // 1b. Qualify names that would otherwise share a slug. An extractor names
    // a page from its own file alone and cannot see the rest of the tree, so
    // this runs here, where every entity is in hand. Ordering is irrelevant to
    // the result — the pass depends on the set of (name, source_path) pairs,
    // not on the order `extracted` arrives in.
    // Project pages first, alone, so a memory note can never rename one; then
    // the notes among themselves with the project ids already taken. An
    // external note is qualified by its virtual `memory/<rel>` path, never by
    // the absolute key folder; its `source_path` is restored afterwards.
    disambiguate_ids(&mut extracted);
    let taken: BTreeSet<String> = extracted.iter().map(|e| e.id.clone()).collect();
    let real_paths: BTreeMap<&String, &String> =
        virtual_paths.iter().map(|(a, v)| (v, a)).collect();
    let mut notes = notes;
    for n in &mut notes {
        if let Some(v) = virtual_paths.get(&n.source_path) {
            n.source_path = v.clone();
        }
    }
    disambiguate_taken(&mut notes, &taken);
    for n in &mut notes {
        if let Some(abs) = real_paths.get(&n.source_path) {
            n.source_path = (*abs).clone();
        }
    }
    extracted.extend(notes);

    // 2. Dedup slug collisions deterministically. After disambiguation ids
    // are unique, so which entity a collision would keep never matters; the
    // result does not depend on the order `extracted` arrives in (project
    // files by rel_path, then notes by folder).
    let mut entities: BTreeMap<String, Entity> = BTreeMap::new();
    for e in extracted {
        match entities.get(&e.id) {
            Some(existing) => {
                eprintln!(
                    "warning: BUG — id '{}' claimed twice after disambiguation; keeping {}, skipping {}. Two entities share a source path, which the walk should make impossible.",
                    e.id, existing.source_path, e.source_path
                );
            }
            None => {
                entities.insert(e.id.clone(), e);
            }
        }
    }

    // 2b. Remap any id colliding with a reserved manifest base name (index,
    // llms, agents, graph) so its page can never clobber — or on
    // case-insensitive filesystems, be clobbered by — a manifest artifact.
    // Runs after slug dedup so graph/render/manifest/lint all see the same,
    // final ids. Processes the (already sorted) BTreeMap in key order so the
    // remap is identical across runs/machines/thread counts.
    let mut entities = remap_reserved_names(entities);
    resolve_supersedes(&mut entities);
    // Needs the final ids and `source_path`s, so it follows the remap.
    wiki_code::pair_declarations(&mut entities);

    // 3. Graph.
    let graph = graph::build_graph(&entities);
    // Reads the import edges, so it follows the graph.
    calls::resolve_calls(&mut entities, &graph);
    let t_global = Instant::now();

    // 4. Incremental cache + parallel render. Only the LOAD is gated: a plain
    // compile re-renders everything, but still records what it wrote (see the
    // unconditional `cache::save` below).
    let mut cache = if opts.incremental {
        cache::load(output)
    } else {
        cache::Cache::fresh()
    };
    std::fs::create_dir_all(output)?;

    // Notes follow a page's SOURCE PATH, not its id: this compile may rename
    // ids (see `disambiguate_ids`), and `<new_id>.md` would not exist. A later
    // pass will use this same index to know which prior page files were
    // consumed by a rename, so the prune can delete them safely.
    //
    // The index is one-to-many — an orphan left by an earlier id scheme goes
    // on declaring the live source until a prune removes it — so every read
    // below picks its file through `rewrite::prior_page_for`, which prefers
    // the page's OWN file whenever it is among the candidates. Read before
    // any page is written this run, and every read below happens inside the
    // parallel map that `.collect()` drains before the write loop starts, so
    // no notes read can observe a file this compile wrote.
    let source_index = rewrite::page_source_index(output);
    let prior_ids = cache::prior_page_ids(output);

    let by_path = rewrite::path_index(&entities);
    let rendered: Vec<(String, String, String, Option<PathBuf>)> = entities
        .par_iter()
        .map(|(id, e)| {
            let edges = graph.edges.get(id).cloned().unwrap_or_default();
            let page_path = output.join(format!("{id}.md"));
            let prior = source_index
                .get(&e.source_path)
                .and_then(|c| rewrite::prior_page_for(c, &page_path));
            let notes = rewrite::read_notes_from(prior.as_deref().unwrap_or(&page_path));
            let fp = hash::to_hex(&rewrite::render_fingerprint(e, &edges, &entities, &notes));
            let content = rewrite::render_page(e, &edges, &entities, &by_path, &notes);
            (id.clone(), content, fp, prior)
        })
        .collect();
    // The prior page files whose notes were migrated onto a live page this run.
    let consumed: BTreeSet<PathBuf> = rendered.iter().filter_map(|r| r.3.clone()).collect();
    let t_render = Instant::now();

    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    let mut written = 0usize;
    for (id, content, fp, _) in rendered {
        if !opts.incremental || cache.needs_render(&id, &fp) {
            rewrite::write_atomic(&output.join(format!("{id}.md")), &content)?;
            written += 1;
        }
        cache.set(&id, &fp);
        pages.insert(id, content);
    }
    let t_write = Instant::now();

    // 5. Prune pages this compiler previously wrote that are no longer live.
    // Runs unconditionally: `--incremental` decides whether to TRUST the cache
    // for re-render decisions, not whether the output directory may keep files
    // that no longer correspond to any source.
    //
    // `consumed` (above) is every prior page file whose notes were migrated onto a
    // (possibly renamed) live page this compile — the file each page read
    // its notes from. A losing
    // candidate is deliberately absent: nothing was migrated off it, so it
    // must reach `prune_stale`'s notes check and be kept-with-warning if it
    // still carries hand-written notes.
    let live: BTreeSet<String> = entities.keys().cloned().collect();
    let mut prunable: BTreeSet<String> = cache.pages.keys().cloned().collect();
    prunable.extend(prior_ids.iter().cloned());
    prune_stale(&prunable, &live, output, &consumed);
    cache.retain_ids(&live);
    // Written on EVERY compile, `--incremental` or not. The cache is the only
    // record of which pages this compiler wrote, and the prune above is only
    // as effective as that record: without this, a workflow that never passes
    // the flag never writes `.wiki/cache.json`, `prior_page_ids` is always
    // empty, `cache.pages` holds exactly the live ids the render loop just
    // set — and the prune, though it runs, has nothing to prune.
    // `cache::load` stays gated on `opts.incremental`, so what changes is what
    // is RECORDED, not what is trusted for re-render decisions.
    cache::save(output, &cache)?;

    // Migration diagnostic: warn (never delete) about pages left by a previous
    // id scheme that survived the prune above. That is USUALLY because they
    // carry unmigrated hand-written notes, but it is not the only exit from
    // `prune_stale` — a `remove_file` that failed on permissions or IO leaves
    // the file here too, so the message states the fact and not a motive.
    let stale = stale_page_files(&prior_ids, &live, output);
    if !stale.is_empty() {
        eprintln!(
            "warning: {} page(s) from a previous id scheme remain in {}: {} — not deleted, either because they carry unmigrated hand-written notes or because removing them failed (see any warning above)",
            stale.len(),
            output.display(),
            stale.join(", ")
        );
    }

    // 6. Manifest artifacts.
    let root = input
        .canonicalize()
        .ok()
        .and_then(|p| Some(p.file_name()?.to_string_lossy().to_string()))
        .unwrap_or_default();
    let man = manifest::build_manifest(&project, &root, &entities, &graph);
    // Ahead of `index.json`, whose change is what tells `serve` to reload,
    // so a reload never pairs a new index with the previous items.
    let put = |name: &str, text: String| rewrite::write_atomic(&output.join(name), &text);
    put("items.json", manifest::render_items_json(&entities))?;
    put("index.json", manifest::render_index_json(&man))?;
    put("index.md", manifest::render_index_md(&man))?;
    put("llms.txt", manifest::render_llms_txt(&man))?;
    put("AGENTS.md", manifest::render_agents_md(&project))?;
    if opts.emit_json {
        put("graph.json", manifest::render_graph_json(&entities, &graph))?;
    }

    // 7. Lint (in-memory — no disk re-read).
    let lint = lint::lint(&pages);

    if timings {
        let us = |a: Instant, b: Instant| (b - a).as_micros();
        let stages = [
            us(t0, t_walk),
            us(t_walk, t_extract),
            us(t_extract, t_global),
            us(t_global, t_render),
            us(t_render, t_write),
            us(t_write, Instant::now()),
        ];
        let ms = |u: u128| format!("{}.{:03}ms", u / 1000, u % 1000);
        eprintln!(
            "timings: total={} walk={} extract={} global={} render={} write={} tail={} files={} written={written}",
            ms(stages.iter().sum()),
            ms(stages[0]),
            ms(stages[1]),
            ms(stages[2]),
            ms(stages[3]),
            ms(stages[4]),
            ms(stages[5]),
            files.len() + memory_files.len(),
        );
    }

    Ok(CompileResult {
        pages_written: written,
        pages_total: entities.len(),
        lint,
        watch_set,
    })
}

/// Page files the compiler previously wrote (per the prior cache) that are no
/// longer live and still exist on disk — orphaned by an id-scheme change.
/// Returned sorted for a deterministic warning; never deleted here.
fn stale_page_files(
    prior: &BTreeSet<String>,
    live: &BTreeSet<String>,
    output: &Path,
) -> Vec<String> {
    prior
        .iter()
        .filter(|id| !live.contains(*id))
        .filter(|id| output.join(format!("{id}.md")).exists())
        .map(|id| format!("{id}.md"))
        .collect()
}

/// Marks each memory note named by another note's `supersedes` with the
/// superseder's id. A name matches a note's title or alias ignoring case, or
/// its id as a slug; project pages are never marked. Several superseders
/// resolve to the smallest id (ids are visited in sorted order).
fn resolve_supersedes(entities: &mut BTreeMap<String, Entity>) {
    let mut marks: BTreeMap<String, String> = BTreeMap::new();
    for (id, e) in entities.iter().filter(|(_, e)| e.memory) {
        for name in &e.supersedes {
            let lower = name.to_lowercase();
            let slug = slugify(name);
            let target = entities.iter().find(|(tid, t)| {
                t.memory
                    && *tid != id
                    // A committed note never retires an uncommitted one.
                    && !(matches!(e.origin, Origin::Repo { .. })
                        && t.origin == Origin::Local)
                    && (**tid == slug
                        || t.name.to_lowercase() == lower
                        || t.aliases.iter().any(|a| a.to_lowercase() == lower))
            });
            if let Some((tid, _)) = target {
                marks.entry(tid.clone()).or_insert_with(|| id.clone());
            }
        }
    }
    for (tid, by) in marks {
        if let Some(t) = entities.get_mut(&tid) {
            t.superseded_by = Some(by);
        }
    }
}

#[cfg(test)]
mod tests;
