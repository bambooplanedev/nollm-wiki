pub mod cache;
pub mod formats;
pub mod generator;
pub mod graph;
pub mod hash;
pub mod lint;
pub mod manifest;
pub mod model;
pub mod query;
pub mod rewrite;
pub mod walk;
pub mod watch;

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
    // the sorted-by-rel_path order that `walk` produced). Files under
    // `output` are excluded so a nested output dir never feeds its own
    // generated pages back in as sources (which would also self-trigger a
    // `--watch` recompile loop).
    let files: Vec<_> = walk::walk(input, opts.respect_ignore)?
        .into_iter()
        .filter(|sf| !is_under(&sf.abs_path, output))
        .collect();
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

    // 2b. Remap any id colliding with a reserved manifest base name (index,
    // llms, agents, graph) so its page can never clobber — or on
    // case-insensitive filesystems, be clobbered by — a manifest artifact.
    // Runs after slug dedup so graph/render/manifest/lint all see the same,
    // final ids. Processes the (already sorted) BTreeMap in key order so the
    // remap is identical across runs/machines/thread counts.
    let entities = remap_reserved_names(entities);

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

/// Base names (case-insensitive) reserved for manifest artifacts written
/// alongside pages: `index.md`/`index.json`, `llms.txt`, `AGENTS.md`, and
/// (with `--emit-json`) `graph.json`.
/// True if `path` lies within `dir`, compared on lexically-absolute components
/// (`std::path::absolute` — no filesystem access, no symlink resolution). This
/// is a predicate only: it never reaches output bytes, so it stays
/// deterministic across machines. Falls back to a raw component compare if
/// absolute-normalization errors (degrade, never panic).
pub(crate) fn is_under(path: &Path, dir: &Path) -> bool {
    match (std::path::absolute(path), std::path::absolute(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => path.starts_with(dir),
    }
}

const RESERVED_MANIFEST_NAMES: [&str; 4] = ["index", "llms", "agents", "graph"];

fn is_reserved_manifest_name(id: &str) -> bool {
    RESERVED_MANIFEST_NAMES.contains(&id.to_lowercase().as_str())
}

/// Rewrite any entity id that collides with a reserved manifest base name to
/// a non-colliding id, deterministically. Ids that already exist and are not
/// themselves reserved are treated as fixed and never displaced; a remapped
/// id gets an `_page` suffix, then `_page_2`, `_page_3`, ... until it clears
/// both the reserved set and every id already claimed (fixed or previously
/// remapped). `entities` is a `BTreeMap`, so iteration is in sorted key
/// order — the same input always produces the same remap, regardless of
/// thread count or machine.
fn remap_reserved_names(entities: BTreeMap<String, Entity>) -> BTreeMap<String, Entity> {
    let mut used: BTreeSet<String> = entities
        .keys()
        .filter(|id| !is_reserved_manifest_name(id))
        .cloned()
        .collect();

    let mut result: BTreeMap<String, Entity> = BTreeMap::new();
    for (id, mut e) in entities {
        if !is_reserved_manifest_name(&id) {
            result.insert(id, e);
            continue;
        }

        let mut candidate = format!("{id}_page");
        let mut suffix = 2;
        while is_reserved_manifest_name(&candidate) || used.contains(&candidate) {
            candidate = format!("{id}_page_{suffix}");
            suffix += 1;
        }

        eprintln!(
            "warning: '{id}' collides with a reserved manifest name — writing page as '{candidate}.md'"
        );

        used.insert(candidate.clone());
        e.id = candidate.clone();
        result.insert(candidate, e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;

    fn ent(id: &str) -> Entity {
        Entity {
            id: id.into(),
            name: id.into(),
            aliases: vec![],
            created: String::new(),
            body: String::new(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
        }
    }

    fn map(ids: &[&str]) -> BTreeMap<String, Entity> {
        ids.iter().map(|id| (id.to_string(), ent(id))).collect()
    }

    #[test]
    fn reserved_id_is_remapped_and_entity_id_field_matches_key() {
        let out = remap_reserved_names(map(&["index", "alpha"]));
        assert!(!out.contains_key("index"));
        assert!(out.contains_key("index_page"));
        assert_eq!(out["index_page"].id, "index_page");
        assert!(out.contains_key("alpha"));
    }

    #[test]
    fn all_reserved_names_are_remapped() {
        let out = remap_reserved_names(map(&["index", "llms", "agents", "graph"]));
        for reserved in ["index", "llms", "agents", "graph"] {
            assert!(!out.contains_key(reserved));
            assert!(out.contains_key(&format!("{reserved}_page")));
        }
    }

    #[test]
    fn remap_avoids_colliding_with_an_existing_fixed_id() {
        // "index_page" is already taken by a real (non-reserved) entity, so
        // "index" must skip straight past it to "index_page_2".
        let out = remap_reserved_names(map(&["index", "index_page"]));
        assert!(!out.contains_key("index"));
        assert!(out.contains_key("index_page_2"));
        assert_eq!(out["index_page_2"].id, "index_page_2");
        // The pre-existing entity keeps its original id untouched.
        assert_eq!(out["index_page"].id, "index_page");
    }

    #[test]
    fn remap_is_deterministic_regardless_of_input_order() {
        let a = remap_reserved_names(map(&["index", "agents", "alpha"]));
        let b = remap_reserved_names(map(&["alpha", "agents", "index"]));
        let ids_a: Vec<&String> = a.keys().collect();
        let ids_b: Vec<&String> = b.keys().collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn is_under_matches_nested_paths_only() {
        use std::path::Path;
        assert!(super::is_under(
            Path::new("/proj/wiki/alpha.md"),
            Path::new("/proj/wiki")
        ));
        assert!(!super::is_under(
            Path::new("/proj/alpha.txt"),
            Path::new("/proj/wiki")
        ));
        // Sibling prefix must not false-match (/proj/wiki vs /proj/wiki2).
        assert!(!super::is_under(
            Path::new("/proj/wiki2/a.md"),
            Path::new("/proj/wiki")
        ));
    }
}
