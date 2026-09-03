//! The `wiki` compiler library: `compile` turns a source tree into a deterministic markdown wiki plus index files.

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
pub mod serve;
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
    #[error("watch failed: {0}")]
    Watch(String),
    #[error("serve failed: {0}")]
    Serve(String),
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

/// Delete the `<id>.md` of every cached page whose id is no longer live.
/// A page that cannot be removed would still be counted by `lint` and
/// served by `serve`, so that is warned about instead of reported as a
/// clean compile. Already-gone is the normal case after a manual delete and
/// needs no warning.
fn prune_stale(cache: &cache::Cache, live: &BTreeSet<String>, output: &Path) {
    for stale in cache.pages.keys().filter(|k| !live.contains(*k)) {
        if let Err(e) = std::fs::remove_file(output.join(format!("{stale}.md"))) {
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
    let registry = Registry::with_defaults();
    let files: Vec<_> = walk::walk(input, opts.respect_ignore, &|p| registry.handles(p))?
        .into_iter()
        .filter(|sf| !is_under(&sf.abs_path, output))
        .collect();
    let mut extracted: Vec<Entity> = files
        .par_iter()
        .filter_map(|sf| registry.extract(&sf.rel_path, &sf.bytes))
        .collect();

    // 1b. Qualify names that would otherwise share a slug. An extractor names
    // a page from its own file alone and cannot see the rest of the tree, so
    // this runs here, where every entity is in hand. Ordering is irrelevant to
    // the result — the pass depends on the set of (name, source_path) pairs,
    // not on the order `extracted` arrives in.
    disambiguate_ids(&mut extracted);

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
    let prior_ids = cache::prior_page_ids(output);

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
        prune_stale(&cache, &live, output);
    }
    cache.retain_ids(&live);
    if opts.incremental {
        cache::save(output, &cache)?;
    }

    // Migration diagnostic: warn (never delete) about pages left by a previous
    // id scheme. Runs after the incremental prune so it never warns about files
    // that prune already removed.
    let stale = stale_page_files(&prior_ids, &live, output);
    if !stale.is_empty() {
        eprintln!(
            "warning: {} page(s) from a previous id scheme remain in {} and are no longer generated: {} — delete them or recompile into a clean directory",
            stale.len(),
            output.display(),
            stale.join(", ")
        );
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

/// Base names (case-insensitive) reserved for manifest artifacts written
/// alongside pages: `index.md`/`index.json`, `llms.txt`, `AGENTS.md`, and
/// (with `--emit-json`) `graph.json`.
const RESERVED_MANIFEST_NAMES: [&str; 4] = ["index", "llms", "agents", "graph"];

fn is_reserved_manifest_name(id: &str) -> bool {
    RESERVED_MANIFEST_NAMES.contains(&id.to_lowercase().as_str())
}

/// The directory segments of `source_path`, outermost first, with the
/// filename dropped.
fn parent_segments(source_path: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = source_path.split('/').filter(|s| !s.is_empty()).collect();
    segs.pop();
    segs
}

/// `base` with the innermost `depth` directory segments prefixed to it:
/// `("Models", "app/api/models.py", 1)` is `"Api Models"`.
fn qualify(base: &str, source_path: &str, depth: usize) -> String {
    if depth == 0 {
        return base.to_string();
    }
    let mut segs = parent_segments(source_path);
    // A directory-module page (`app/api/__init__.py`, `a/common/mod.rs`) is
    // already named for its nearest directory, so prepending that same segment
    // would render "Api Api" — a name whose tokens appear in no other page's
    // body, which is exactly the unreachability the naming rule exists to fix.
    // Qualify from the segment above it instead.
    if segs
        .last()
        .map(|s| crate::model::title_case(&s.replace(['_', '-'], " ")))
        .as_deref()
        == Some(base)
    {
        segs.pop();
    }
    let start = segs.len().saturating_sub(depth);
    let mut name = String::new();
    for seg in &segs[start..] {
        name.push_str(&crate::model::title_case(&seg.replace(['_', '-'], " ")));
        name.push(' ');
    }
    name.push_str(base);
    name
}

/// Give every page a unique id by prefixing parent directory segments to its
/// name until no two entities share a slug.
///
/// A page is named after its file's basename alone, so the conventional
/// layouts all collapse onto a single slug — `app/{api,db,web}/models.py`,
/// `src/*/mod.rs`, and every `__init__.py` in a package tree. Without this
/// pass the collision loop in `compile_inner` drops every file but the first
/// with nothing louder than a stderr warning: a seven-file Django-shaped tree
/// compiled to two pages while the run still exited 0 and lint reported no
/// broken links.
///
/// Qualification is collision-driven, not unconditional: a name that is
/// already unique is never touched, so the short titles the wiki is readable
/// with survive, and a qualified title is always evidence of a real clash.
///
/// The loop extends every member of a colliding group by one segment per
/// round, which is what lets a group separate even when a shorter prefix is
/// itself ambiguous (`a/x/mod.rs` and `b/x/mod.rs` both qualify to `X Mod`
/// before the second round reaches `A X Mod` / `B X Mod`). It terminates
/// because a path has finitely many segments and a round that extends nobody
/// breaks out — which is also what happens to a file at the project root,
/// where there is no segment left to take. That case, and two files with
/// genuinely identical paths, still fall through to the first-wins dedup in
/// `compile_inner`, which stays as the backstop.
///
/// Renaming a page also changes what `graph::build_graph` matches against
/// body text: `Api Models` is a two-token phrase where `Models` was one, so a
/// qualified page picks up fewer lexical edges than it would have. That is a
/// trade against pages that previously did not exist at all.
fn disambiguate_ids(extracted: &mut [Entity]) {
    let base: Vec<String> = extracted.iter().map(|e| e.name.clone()).collect();
    let mut depth = vec![0usize; extracted.len()];
    loop {
        let mut by_id: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, e) in extracted.iter().enumerate() {
            by_id.entry(e.id.as_str()).or_default().push(i);
        }
        let clashing: Vec<usize> = by_id
            .into_values()
            .filter(|g| g.len() > 1)
            .flatten()
            .collect();

        let mut extended = Vec::new();
        for i in clashing {
            if depth[i] < parent_segments(&extracted[i].source_path).len() {
                depth[i] += 1;
                extended.push(i);
            }
        }
        if extended.is_empty() {
            return;
        }
        for i in extended {
            let name = qualify(&base[i], &extracted[i].source_path, depth[i]);
            extracted[i].id = crate::model::slugify(&name);
            extracted[i].name = name;
        }
    }
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
            defined: vec![],
        }
    }

    fn map(ids: &[&str]) -> BTreeMap<String, Entity> {
        ids.iter().map(|id| (id.to_string(), ent(id))).collect()
    }

    fn ent_at(name: &str, path: &str) -> Entity {
        let mut e = ent(name);
        e.id = crate::model::slugify(name);
        e.name = name.into();
        e.source_path = path.into();
        e
    }

    fn ids_after_disambiguation(pairs: &[(&str, &str)]) -> Vec<String> {
        let mut v: Vec<Entity> = pairs.iter().map(|(n, p)| ent_at(n, p)).collect();
        disambiguate_ids(&mut v);
        v.into_iter().map(|e| e.id).collect()
    }

    #[test]
    fn colliding_basenames_are_qualified_by_their_parent_directory() {
        // The canonical Python app layout: every package has its own
        // `models.py`, and before qualification all but the first were
        // dropped outright by the dedup below.
        assert_eq!(
            ids_after_disambiguation(&[
                ("Models", "app/api/models.py"),
                ("Models", "app/db/models.py"),
                ("Models", "app/web/models.py"),
            ]),
            vec!["api_models", "db_models", "web_models"]
        );
    }

    #[test]
    fn a_unique_basename_is_left_alone() {
        // Qualification is collision-driven: a page that never collides keeps
        // the short title, so a rename is always evidence of a real clash.
        assert_eq!(
            ids_after_disambiguation(&[
                ("Graph", "src/graph.rs"),
                ("Models", "app/api/models.py"),
                ("Store", "app/db/store.py"),
            ]),
            vec!["graph", "models", "store"]
        );
    }

    #[test]
    fn qualification_extends_until_unique() {
        // One directory is not enough here — both files sit under a `mod`
        // directory named `x`, so the walk has to take a second segment.
        assert_eq!(
            ids_after_disambiguation(&[("Mod", "a/x/mod.rs"), ("Mod", "b/x/mod.rs")]),
            vec!["a_x_mod", "b_x_mod"]
        );
    }

    #[test]
    fn a_root_file_that_cannot_extend_keeps_its_name() {
        // A file at the project root has no segment left to prefix. It keeps
        // the bare id and the file that *can* extend moves aside, so the pair
        // still separates without either page being dropped.
        assert_eq!(
            ids_after_disambiguation(&[("Readme", "README.md"), ("Readme", "docs/README.md")]),
            vec!["readme", "docs_readme"]
        );
    }

    #[test]
    fn qualification_preserves_a_content_derived_title() {
        // Markdown and text pages take their name from an H1 or frontmatter
        // title, not from the path. Qualification must PREFIX that title, not
        // replace it with the filename.
        let mut v = vec![
            ent_at("Overview", "docs/api/index.md"),
            ent_at("Overview", "docs/db/index.md"),
        ];
        disambiguate_ids(&mut v);
        assert_eq!(v[0].name, "Api Overview");
        assert_eq!(v[1].name, "Db Overview");
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
    fn stale_page_files_lists_orphaned_on_disk_only() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path();
        std::fs::write(out.join("old_slug.md"), "x").unwrap();
        std::fs::write(out.join("kept.md"), "x").unwrap();
        let prior: BTreeSet<String> = ["old_slug", "kept", "never_written"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let live: BTreeSet<String> = ["kept", "new_slug"].iter().map(|s| s.to_string()).collect();
        // old_slug: prior, not live, on disk → listed.
        // kept: live → excluded. never_written: not on disk → excluded.
        assert_eq!(
            super::stale_page_files(&prior, &live, out),
            vec!["old_slug.md".to_string()]
        );
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
