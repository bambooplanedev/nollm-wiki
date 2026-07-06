pub mod cache;
pub mod formats;
pub mod graph;
pub mod hash;
pub mod lint;
pub mod manifest;
pub mod model;
pub mod query;
pub mod rewrite;
pub mod walk;

use crate::formats::Registry;
use crate::model::{Entity, LintReport};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum WikiError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("rayon pool build failed: {0}")]
    Pool(String),
}

pub struct CompileOptions {
    pub incremental: bool,
    pub respect_ignore: bool,
    pub emit_json: bool,
    pub jobs: Option<usize>,
    pub project: Option<String>,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            incremental: false,
            respect_ignore: true,
            emit_json: false,
            jobs: None,
            project: None,
        }
    }
}

pub struct CompileResult {
    pub pages_written: usize,
    pub pages_total: usize,
    pub lint: LintReport,
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

fn compile_inner(
    input: &Path,
    output: &Path,
    opts: &CompileOptions,
) -> Result<CompileResult, WikiError> {
    let project = opts.project.clone().unwrap_or_else(|| {
        input
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "wiki".into())
    });

    // 1. Walk + parallel extract (ordered — collecting into a Vec preserves
    // the sorted-by-rel_path order that `walk` produced).
    let files = walk::walk(input, opts.respect_ignore)?;
    let registry = Registry::with_defaults();
    let extracted: Vec<Entity> = files
        .par_iter()
        .filter_map(|sf| registry.extract(&sf.rel_path, &sf.bytes))
        .collect();

    // 2. Dedup slug collisions deterministically (first by sorted rel_path
    // wins — `extracted` is already ordered by rel_path from the walk, so a
    // simple "keep the first insertion" rule is order-stable regardless of
    // thread count).
    let mut entities: BTreeMap<String, Entity> = BTreeMap::new();
    for e in extracted {
        match entities.get(&e.id) {
            Some(existing) => {
                eprintln!(
                    "warning: slug collision '{}' — keeping {}, skipping {}",
                    e.id, existing.source_path, e.source_path
                );
            }
            None => {
                entities.insert(e.id.clone(), e);
            }
        }
    }

    // 3. Graph.
    let graph = graph::build_graph(&entities);

    // 4. Incremental cache + parallel render.
    let mut cache = if opts.incremental {
        cache::load(output)
    } else {
        cache::Cache::fresh()
    };
    std::fs::create_dir_all(output)?;

    let rendered: Vec<(String, String, String)> = entities
        .par_iter()
        .map(|(id, e)| {
            let edges = graph.edges.get(id).cloned().unwrap_or_default();
            let page_path = output.join(format!("{id}.md"));
            let notes = rewrite::read_preserved_notes(&page_path);
            let fp = hash::to_hex(&rewrite::render_fingerprint(e, &edges, &entities, &notes));
            let content = rewrite::render_page(e, &edges, &entities, &notes);
            (id.clone(), content, fp)
        })
        .collect();

    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    let mut written = 0usize;
    for (id, content, fp) in rendered {
        if !opts.incremental || cache.needs_render(&id, &fp) {
            rewrite::write_atomic(&output.join(format!("{id}.md")), &content)?;
            written += 1;
        }
        cache.set(&id, &fp);
        pages.insert(id, content);
    }

    // 5. Prune deleted pages from cache + disk.
    let live: BTreeSet<String> = entities.keys().cloned().collect();
    if opts.incremental {
        for stale in cache
            .pages
            .keys()
            .filter(|k| !live.contains(*k))
            .cloned()
            .collect::<Vec<_>>()
        {
            let _ = std::fs::remove_file(output.join(format!("{stale}.md")));
        }
    }
    cache.retain_ids(&live);
    if opts.incremental {
        cache::save(output, &cache)?;
    }

    // 6. Manifest artifacts.
    let man = manifest::build_manifest(&project, &entities, &graph);
    rewrite::write_atomic(
        &output.join("index.json"),
        &manifest::render_index_json(&man),
    )?;
    rewrite::write_atomic(&output.join("index.md"), &manifest::render_index_md(&man))?;
    rewrite::write_atomic(&output.join("llms.txt"), &manifest::render_llms_txt(&man))?;
    rewrite::write_atomic(
        &output.join("AGENTS.md"),
        &manifest::render_agents_md(&project),
    )?;
    if opts.emit_json {
        rewrite::write_atomic(
            &output.join("graph.json"),
            &manifest::render_graph_json(&entities, &graph),
        )?;
    }

    // 7. Lint (in-memory — no disk re-read).
    let lint = lint::lint(&pages);

    Ok(CompileResult {
        pages_written: written,
        pages_total: entities.len(),
        lint,
    })
}
