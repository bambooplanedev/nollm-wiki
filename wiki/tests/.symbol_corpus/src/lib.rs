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
    consumed: &BTreeSet<std::path::PathBuf>,
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

    let project = opts.project.clone().unwrap_or_else(|| {
        input
            .file_name()
            .map_or_else(|| "wiki".into(), |s| s.to_string_lossy().to_string())
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
    let entities = remap_reserved_names(entities);

    // 3. Graph.
    let graph = graph::build_graph(&entities);

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

    let rendered: Vec<(String, String, String)> = entities
        .par_iter()
        .map(|(id, e)| {
            let edges = graph.edges.get(id).cloned().unwrap_or_default();
            let page_path = output.join(format!("{id}.md"));
            let prior = source_index
                .get(&e.source_path)
                .and_then(|c| rewrite::prior_page_for(c, &page_path));
            let notes = rewrite::read_notes_from(prior.as_deref().unwrap_or(&page_path));
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

    // 5. Prune pages this compiler previously wrote that are no longer live.
    // Runs unconditionally: `--incremental` decides whether to TRUST the cache
    // for re-render decisions, not whether the output directory may keep files
    // that no longer correspond to any source.
    //
    // `consumed` is every prior page file whose notes were migrated onto a
    // (possibly renamed) live page this compile — the SAME file each page read
    // its notes from above, chosen by the same `prior_page_for` rule. A losing
    // candidate is deliberately absent: nothing was migrated off it, so it
    // must reach `prune_stale`'s notes check and be kept-with-warning if it
    // still carries hand-written notes.
    let live: BTreeSet<String> = entities.keys().cloned().collect();
    let consumed: BTreeSet<std::path::PathBuf> = entities
        .values()
        .filter_map(|e| {
            let page_path = output.join(format!("{}.md", e.id));
            source_index
                .get(&e.source_path)
                .and_then(|c| rewrite::prior_page_for(c, &page_path))
        })
        .collect();
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

/// The token that tells two pages apart when directory prefixing cannot.
///
/// A code page is named after `module_stem`, so what distinguishes it is
/// built from the dotted segments that stem discarded: the segments BETWEEN
/// the stem and the final extension, joined by `_` — `client.test.ts` ->
/// `test`, `foo.test.spec.ts` -> `test_spec`. When there is no infix
/// (`orderbook.h`, `orderbook.cpp`, `client.ts`), the final extension itself
/// is the discriminator.
///
/// The full dotted tail (`test.ts`, `test.spec.ts`) is deliberately NOT used:
/// it would name `client.test.ts` "Client Test Ts", and the token "Ts"
/// appears in no other page's body — the same unreachable-by-mention defect
/// the redundancy guard in `disambiguate_ids` exists to prevent, and the same
/// rationale `qualify` already carries for a directory-module page above.
///
/// A prose page's name comes from its own `# H1` or frontmatter title and has
/// nothing to do with the filename, so the basename STEM is what
/// distinguishes it — `v1`, not `v1_md`, which would name nothing. A
/// directory-module page (`pkg/__init__.py`) discarded no tail and uses the
/// stem for the same reason.
fn discriminator(entity: &Entity) -> String {
    let base = entity
        .source_path
        .rsplit('/')
        .next()
        .unwrap_or(&entity.source_path);
    let stem = base.split('.').next().unwrap_or(base);
    let named_after_stem = matches!(entity.kind, crate::model::SourceKind::Code { .. })
        && crate::formats::code::module_stem(&entity.source_path) == stem;
    if named_after_stem {
        let mut segs: Vec<&str> = base.split('.').collect();
        if segs.len() > 1 {
            segs.remove(0); // the stem itself
            let ext = segs.pop().unwrap_or(""); // the final extension
            let token = if segs.is_empty() {
                ext.to_string()
            } else {
                segs.join("_")
            };
            if !token.is_empty() {
                return crate::model::slugify(&token);
            }
        }
    }
    crate::model::slugify(stem)
}

/// The name an entity renders at a given rung of the ladder.
fn render_name(base: &str, source_path: &str, depth: usize, disc: &str, ordinal: usize) -> String {
    let mut name = qualify(base, source_path, depth);
    if !disc.is_empty() {
        name = format!(
            "{name} {}",
            crate::model::title_case(&disc.replace('_', " "))
        );
    }
    if ordinal > 0 {
        name = format!("{name} {ordinal}");
    }
    name
}

/// The group member exempt from discrimination — the unique minimum of
/// `(source_rank, dot count)`. `None` when the minimum is achieved more than
/// once: a tie means nothing makes a member canonical, so nobody keeps the
/// bare name. Without this, adding a generated `utils.js` beside an existing
/// `utils.ts` would hand the build artifact the canonical `utils` id.
fn strict_winner(group: &[usize], key: &[(u8, usize)]) -> Option<usize> {
    let min = group.iter().map(|&i| key[i]).min()?;
    let mut at_min = group.iter().copied().filter(|&i| key[i] == min);
    let first = at_min.next()?;
    at_min.next().is_none().then_some(first)
}

/// True if applying `disc` to `current` would double a token already there:
/// `current`'s own trailing words already spell out `disc`'s words. `disc`
/// can carry more than one word (`test_spec`), so this checks that many
/// trailing words of `current`, not just the last one — comparing only the
/// last token would miss "Foo Test Spec" + `test_spec`, which renders "Foo
/// Test Spec Test Spec".
fn discriminator_is_redundant(current: &str, disc: &str) -> bool {
    let disc_words: Vec<String> = disc
        .split('_')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    if disc_words.is_empty() {
        return false;
    }
    let current_words: Vec<&str> = current.split(' ').collect();
    if current_words.len() < disc_words.len() {
        return false;
    }
    let tail = &current_words[current_words.len() - disc_words.len()..];
    tail.iter().map(|w| w.to_lowercase()).eq(disc_words)
}

/// Per-entity state `assign_free_ids` renders from but never mutates: the
/// pre-discriminator base name, the discriminator itself, the qualification
/// depth, and whether that entity has actually used its discriminator.
/// Bundled so the function stays under clippy's argument-count lint instead
/// of taking four parallel slices.
struct LadderState<'a> {
    base: &'a [String],
    disc: &'a [String],
    depth: &'a [usize],
    used_disc: &'a [bool],
}

/// Assign every member of `group` a distinct, previously-unclaimed id: the
/// last rung of the ladder, and the reason it cannot fail.
///
/// Members are visited in `source_path` order — a strict total order over
/// distinct files — and each searches `0, 2, 3, 4, …` for the smallest
/// ordinal whose rendered id is not in `claimed`, INCLUDING position 0: a
/// member sharing a colliding id with others is not entitled to keep any
/// particular ordinal from a previous round just because it rendered first
/// last time, and position 0 needs the same fresh search as everyone else to
/// clear a value that only made sense against a blocker which has since
/// moved. `claimed` is threaded through the whole round (not reset per
/// group), so two groups processed in the same round never land on the same
/// id by both computing from the round's starting snapshot.
fn assign_free_ids(
    group: &[usize],
    extracted: &[Entity],
    state: &LadderState<'_>,
    ordinal: &mut [usize],
    claimed: &mut BTreeSet<String>,
) -> bool {
    // Every member of `group` currently shares this exact id (that is what
    // makes them a group) — vacate it once, for the whole group, rather than
    // per member: removing it again for a later member would just be
    // removing whatever a not-yet-processed sibling still has not replaced.
    claimed.remove(&extracted[group[0]].id);

    let mut sorted = group.to_vec();
    sorted.sort_by(|a, b| extracted[*a].source_path.cmp(&extracted[*b].source_path));

    let mut changed = false;
    for i in sorted {
        let d = if state.used_disc[i] {
            state.disc[i].as_str()
        } else {
            ""
        };
        let mut n = 0usize;
        let new_id = loop {
            let candidate = render_name(
                &state.base[i],
                &extracted[i].source_path,
                state.depth[i],
                d,
                n,
            );
            let candidate_id = crate::model::slugify(&candidate);
            if !claimed.contains(&candidate_id) {
                break candidate_id;
            }
            n = if n == 0 { 2 } else { n + 1 };
        };
        if ordinal[i] != n {
            ordinal[i] = n;
            changed = true;
        }
        claimed.insert(new_id);
    }
    changed
}

/// Give every page a unique id, always.
///
/// An escalation ladder, one monotonic state per entity: prefix parent
/// directory segments while that can separate the group; then append the
/// filename discriminator, once; then a free-id search over sorted
/// `source_path` — the ids already claimed this round, not a position in the
/// group. That last rung is what cannot fail: `assign_free_ids` searches
/// `0, 2, 3, 4, …` and the id space is unbounded, so a free id always exists.
/// It is not "an ordinal over sorted `source_path`" in the sense of a position
/// -> number mapping — a member's OWN root name can independently already
/// render an ordinal-shaped id (`docs/2.md` titled "Chapter 2" sits beside a
/// `docs/chapter.md` colliding with `docs/Chapter.md`), so which numbers are
/// actually free can only be answered by checking every current id, not by
/// counting the group's own members.
///
/// Termination is bounded on every rung — depth by the path's segment count,
/// the discriminator by its once-only flag, and the free-id search because
/// each round searches ids fresh from `claimed` rather than incrementing a
/// remembered value, so a name never grows `_2_2_2`. The outer loop still
/// carries a belt-and-braces round cap (see below): the per-rung arguments
/// above are why it should never be reached.
///
/// Qualification stays collision-driven: a name that is already unique is
/// never touched, so the short titles the wiki is readable with survive and a
/// qualified title is always evidence of a real clash.
///
/// Renaming a page also changes what `graph::build_graph` matches against
/// body text: `Api Models` is a two-token phrase where `Models` was one, so a
/// qualified page picks up fewer lexical edges. That is the trade against
/// pages that previously did not exist at all.
fn disambiguate_ids(extracted: &mut [Entity]) {
    let base: Vec<String> = extracted.iter().map(|e| e.name.clone()).collect();
    let disc: Vec<String> = extracted.iter().map(discriminator).collect();
    let key: Vec<(u8, usize)> = extracted
        .iter()
        .map(|e| {
            let basename = e.source_path.rsplit('/').next().unwrap_or(&e.source_path);
            (
                crate::formats::code::source_rank(&e.source_path),
                basename.matches('.').count(),
            )
        })
        .collect();

    let mut depth = vec![0usize; extracted.len()];
    let mut used_disc = vec![false; extracted.len()];
    let mut ordinal = vec![0usize; extracted.len()];

    // Belt-and-braces: every rung above terminates on its own (see the doc
    // comment), so this should never be reached. If it ever is — a future
    // change breaks one of those arguments — the compiler must still emit a
    // complete, deterministic wiki rather than loop forever, so the LAST
    // round forces every still-colliding group straight to the free-id
    // search (skipping depth/discriminator) and stops.
    let max_rounds = 2 * extracted.len() + 16;

    for round in 0..=max_rounds {
        let force_free_ids = round == max_rounds;

        let mut by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in extracted.iter().enumerate() {
            by_id.entry(e.id.clone()).or_default().push(i);
        }

        // Threaded through every group this round (not reset per group): two
        // groups processed in the same round must not both compute a free id
        // from the round's starting snapshot and land on the same choice.
        let mut claimed: BTreeSet<String> = extracted.iter().map(|e| e.id.clone()).collect();

        let mut changed = false;
        for group in by_id.values().filter(|g| g.len() > 1) {
            if !force_free_ids {
                let first_parents = parent_segments(&extracted[group[0]].source_path);
                let same_dir = group
                    .iter()
                    .all(|&i| parent_segments(&extracted[i].source_path) == first_parents);
                let can_deepen = group
                    .iter()
                    .any(|&i| depth[i] < parent_segments(&extracted[i].source_path).len());

                if !same_dir && can_deepen {
                    for &i in group {
                        if depth[i] < parent_segments(&extracted[i].source_path).len() {
                            depth[i] += 1;
                            changed = true;
                        }
                    }
                    continue;
                }

                let winner = strict_winner(group, &key);
                let mut applied = false;
                for &i in group {
                    if Some(i) == winner || used_disc[i] || disc[i].is_empty() {
                        continue;
                    }
                    // Redundancy guard: "Foo Test Spec" + disc `test_spec`
                    // would render "Foo Test Spec Test Spec", a doubled tail
                    // that appears in no other page's body — the same shape
                    // `qualify` guards against for directories.
                    let current = render_name(&base[i], &extracted[i].source_path, depth[i], "", 0);
                    if discriminator_is_redundant(&current, &disc[i]) {
                        continue;
                    }
                    used_disc[i] = true;
                    applied = true;
                    changed = true;
                }
                if applied {
                    continue;
                }
            }

            let state = LadderState {
                base: &base,
                disc: &disc,
                depth: &depth,
                used_disc: &used_disc,
            };
            if assign_free_ids(group, extracted, &state, &mut ordinal, &mut claimed) {
                changed = true;
            }
        }

        if !changed {
            return;
        }

        for i in 0..extracted.len() {
            let d = if used_disc[i] { disc[i].as_str() } else { "" };
            let name = render_name(&base[i], &extracted[i].source_path, depth[i], d, ordinal[i]);
            extracted[i].id = crate::model::slugify(&name);
            extracted[i].name = name;
        }

        if force_free_ids {
            return;
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
        e.id.clone_from(&candidate);
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
            methods: vec![],
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

    fn code_entity(path: &str) -> Entity {
        let mut e = ent(path);
        e.source_path = path.to_string();
        e.kind = crate::model::SourceKind::Code { lang: "cpp".into() };
        e
    }

    fn text_entity(path: &str) -> Entity {
        let mut e = ent(path);
        e.source_path = path.to_string();
        e.kind = crate::model::SourceKind::Markdown;
        e
    }

    #[test]
    fn discriminator_is_the_tail_for_code_and_the_stem_for_prose() {
        // No infix between stem and final extension: the extension itself.
        assert_eq!(discriminator(&code_entity("src/orderbook.h")), "h");
        assert_eq!(discriminator(&code_entity("src/orderbook.cpp")), "cpp");
        assert_eq!(discriminator(&code_entity("api/client.ts")), "ts");
        // An infix: the segments between stem and final extension, joined by
        // `_` — NOT the full dotted tail ("test.ts"/"test.spec.ts"), which
        // would put the final extension into the rendered name as its own
        // word ("Client Test Ts") — a token that appears in no other page's
        // body.
        assert_eq!(discriminator(&code_entity("api/client.test.ts")), "test");
        assert_eq!(
            discriminator(&code_entity("api/foo.test.spec.ts")),
            "test_spec"
        );
        // Directory module: module_stem returns "pkg", not the basename, so the
        // tail is not what distinguishes it.
        assert_eq!(discriminator(&code_entity("pkg/__init__.py")), "init");
        // Prose: the name came from the document title, so the stem distinguishes.
        assert_eq!(discriminator(&text_entity("docs/v1.md")), "v1");
        assert_eq!(discriminator(&text_entity("docs/v2.md")), "v2");
    }

    /// Run the pass and return (id, name) pairs in input order.
    fn ids_of(mut ents: Vec<Entity>) -> Vec<(String, String)> {
        for e in &mut ents {
            e.id = crate::model::slugify(&e.name);
        }
        disambiguate_ids(&mut ents);
        ents.into_iter().map(|e| (e.id, e.name)).collect()
    }

    fn named_code(path: &str, name: &str) -> Entity {
        let mut e = code_entity(path);
        e.name = name.to_string();
        e
    }

    #[test]
    fn header_keeps_the_short_name_and_impl_takes_the_discriminator() {
        let got = ids_of(vec![
            named_code("src/orderbook.cpp", "Orderbook"),
            named_code("src/orderbook.h", "Orderbook"),
        ]);
        assert_eq!(got[1].0, "orderbook", "the .h must keep the bare slug");
        assert_eq!(got[0].0, "orderbook_cpp");
    }

    #[test]
    fn three_way_header_group_keeps_a_strict_winner() {
        let got = ids_of(vec![
            named_code("src/ob.h", "Ob"),
            named_code("src/ob.hpp", "Ob"),
            named_code("src/ob.cpp", "Ob"),
        ]);
        assert_eq!(got[0].0, "ob", ".h alone is rank 0, so it wins strictly");
        assert_eq!(got[1].0, "ob_hpp");
        assert_eq!(got[2].0, "ob_cpp");
    }

    #[test]
    fn a_tie_exempts_nobody() {
        let got = ids_of(vec![
            named_code("src/utils.js", "Utils"),
            named_code("src/utils.ts", "Utils"),
        ]);
        assert_eq!(got[0].0, "utils_js");
        assert_eq!(got[1].0, "utils_ts");
        assert!(
            !got.iter().any(|(id, _)| id == "utils"),
            "a build artifact must not take the canonical name: {got:?}"
        );
    }

    #[test]
    fn test_sibling_loses_to_the_plain_source() {
        let got = ids_of(vec![
            named_code("api/client.test.ts", "Client"),
            named_code("api/client.ts", "Client"),
        ]);
        assert_eq!(got[1].0, "client");
        assert_eq!(got[0].0, "client_test");
    }

    #[test]
    fn directory_module_and_sibling_both_survive() {
        let got = ids_of(vec![
            named_code("src/foo.rs", "Foo"),
            named_code("src/foo/mod.rs", "Foo"),
        ]);
        let ids: Vec<_> = got.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(
            ids[0], ids[1],
            "src/foo.rs and src/foo/mod.rs both need a page: {got:?}"
        );
    }

    #[test]
    fn ordinals_rescue_a_group_the_discriminator_cannot_split() {
        // Both basenames slugify to the same stem AND the same tail.
        let got = ids_of(vec![
            named_code("a/foo-bar.py", "Foo Bar"),
            named_code("a/foo_bar.py", "Foo Bar"),
        ]);
        assert_ne!(
            got[0].0, got[1].0,
            "ordinal rung must separate them: {got:?}"
        );
    }

    #[test]
    fn case_variants_get_two_pages_not_one() {
        let got = ids_of(vec![
            named_code("src/Book.h", "Book"),
            named_code("src/book.h", "Book"),
        ]);
        assert_ne!(
            got[0].0, got[1].0,
            "two distinct files need two pages: {got:?}"
        );
    }

    #[test]
    fn redundant_discriminator_is_skipped() {
        let mut prose = text_entity("a/orderbook.md");
        prose.name = "Orderbook".into();
        let got = ids_of(vec![named_code("a/orderbook.h", "Orderbook"), prose]);
        assert!(
            !got.iter().any(|(_, n)| n.contains("Orderbook Orderbook")),
            "doubled token makes the page unreachable by mention: {got:?}"
        );
        assert_ne!(got[0].0, got[1].0);
    }

    #[test]
    fn every_id_is_the_slug_of_its_name() {
        let got = ids_of(vec![
            named_code("a/foo-bar.py", "Foo Bar"),
            named_code("a/foo_bar.py", "Foo Bar"),
            named_code("src/ob.h", "Ob"),
            named_code("src/ob.cpp", "Ob"),
        ]);
        for (id, name) in &got {
            assert_eq!(*id, crate::model::slugify(name), "id/name drift on {name}");
        }
    }

    #[test]
    fn result_does_not_depend_on_input_order() {
        let make = || {
            vec![
                named_code("a/foo.h", "Foo"),
                named_code("a/foo.cpp", "Foo"),
                named_code("b/foo.h", "Foo"),
                named_code("b/foo.cpp", "Foo"),
                named_code("a/foo-x.py", "Foo X"),
                named_code("a/foo_x.py", "Foo X"),
            ]
        };
        let mut baseline: Vec<(String, String)> = ids_of(make())
            .into_iter()
            .zip(make())
            .map(|((id, _), e)| (e.source_path, id))
            .collect();
        baseline.sort();

        // Every rotation of the input must produce the same source_path -> id map.
        for rot in 1..6 {
            let mut ents = make();
            ents.rotate_left(rot);
            let paths: Vec<String> = ents.iter().map(|e| e.source_path.clone()).collect();
            let mut got: Vec<(String, String)> = ids_of(ents)
                .into_iter()
                .zip(paths)
                .map(|((id, _), p)| (p, id))
                .collect();
            got.sort();
            assert_eq!(got, baseline, "rotation {rot} changed the result");
        }
    }

    fn named_text(path: &str, name: &str) -> Entity {
        let mut e = text_entity(path);
        e.name = name.to_string();
        e
    }

    #[test]
    fn four_way_title_collision_all_survive() {
        // docs/2.md's OWN title already renders the ordinal-shaped id
        // "chapter_2" — a coincidence the free-id search must discover by
        // checking every current id, not by counting positions within the
        // {Chapter.md, chapter.md} group. The old positional code rendered
        // "chapter_2" for chapter.md without checking, colliding with
        // docs/2.md and losing a page to the first-wins dedup.
        let got = ids_of(vec![
            named_text("docs/1.md", "Chapter 1"),
            named_text("docs/2.md", "Chapter 2"),
            named_text("docs/Chapter.md", "chapter"),
            named_text("docs/chapter.md", "Chapter"),
        ]);
        let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            4,
            "all four files must get distinct pages: {got:?}"
        );
    }

    #[test]
    fn free_id_search_clears_a_stale_ordinal_at_position_zero() {
        // Simulate a group entering the free-id search with position 0
        // (a/foo.h, first by source_path) carrying an ordinal left over from
        // an earlier round — e.g. it once shared this same id string with a
        // blocker that has since moved away. Nothing blocks the bare name
        // now, so the search must clear it rather than leave it (the old
        // positional code never even visited position 0, so a stale value
        // there was never cleared).
        let extracted = vec![named_code("a/foo.h", "Foo"), named_code("a/foo.hpp", "Foo")];
        let base: Vec<String> = extracted.iter().map(|e| e.name.clone()).collect();
        let disc = vec![String::new(), String::new()];
        let depth = vec![0usize, 0usize];
        let used_disc = vec![false, false];
        let mut ordinal = vec![2usize, 0usize]; // stale: position 0 already at 2
        let mut claimed: BTreeSet<String> = extracted.iter().map(|e| e.id.clone()).collect();

        let state = LadderState {
            base: &base,
            disc: &disc,
            depth: &depth,
            used_disc: &used_disc,
        };
        let changed = assign_free_ids(&[0, 1], &extracted, &state, &mut ordinal, &mut claimed);

        assert!(changed);
        assert_eq!(
            ordinal[0], 0,
            "position 0's stale ordinal must be cleared once nothing blocks the bare name"
        );
        assert_eq!(ordinal[1], 2);
    }

    #[test]
    fn discriminator_redundancy_checks_every_word_the_discriminator_carries() {
        // A single-word discriminator: unchanged behavior.
        assert!(discriminator_is_redundant("Orderbook", "orderbook"));
        assert!(!discriminator_is_redundant("Orderbook", "cpp"));
        // A multi-word discriminator: comparing only the LAST token would
        // wrongly call this non-redundant ("spec" != "test spec").
        assert!(discriminator_is_redundant("Foo Test Spec", "test_spec"));
        assert!(!discriminator_is_redundant("Foo Bar", "test_spec"));
        assert!(!discriminator_is_redundant("Foo Spec", "test_spec"));
    }

    #[test]
    fn multi_word_redundant_discriminator_is_skipped() {
        // foo.test.spec.ts's discriminator (Task 4's infix rule) is
        // "test_spec". Its own name already ends in "Test Spec", so applying
        // the discriminator would render "Foo Test Spec Test Spec" — a
        // doubled tail unreachable by mention, exactly what the guard
        // exists to prevent now that multi-word discriminators are
        // reachable.
        let mut other = text_entity("a/other.md");
        other.name = "Foo Test Spec".into();
        let got = ids_of(vec![
            named_code("a/foo.test.spec.ts", "Foo Test Spec"),
            other,
        ]);
        assert!(
            !got.iter().any(|(_, n)| n.contains("Test Spec Test Spec")),
            "doubled multi-word tail makes the page unreachable by mention: {got:?}"
        );
        assert_ne!(got[0].0, got[1].0);
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
            .map(std::string::ToString::to_string)
            .collect();
        let live: BTreeSet<String> = ["kept", "new_slug"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        // old_slug: prior, not live, on disk → listed.
        // kept: live → excluded. never_written: not on disk → excluded.
        assert_eq!(
            super::stale_page_files(&prior, &live, out),
            vec!["old_slug.md".to_string()]
        );
    }

    #[test]
    fn prune_stale_never_deletes_hand_written_notes() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path();
        std::fs::write(
            out.join("noted.md"),
            "# Noted\n\n## Body\nx\n\n## Notes\nwhy this module is odd\n",
        )
        .unwrap();
        std::fs::write(
            out.join("plain.md"),
            "# Plain\n\n## Body\nx\n\n## Notes\n_(add your own notes here — preserved on recompile)_\n",
        )
        .unwrap();
        std::fs::write(
            out.join("migrated.md"),
            "# Migrated\n\n## Body\nx\n\n## Notes\ncarried onto the renamed page\n",
        )
        .unwrap();
        let prunable: BTreeSet<String> = ["noted", "plain", "migrated"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let consumed: BTreeSet<std::path::PathBuf> = [out.join("migrated.md")].into();
        // Every id is gone from the corpus — an id-scheme change renames every
        // page at once, which is exactly when notes are easiest to lose.
        super::prune_stale(&prunable, &BTreeSet::new(), out, &consumed);
        assert!(
            out.join("noted.md").exists(),
            "the only human-owned section in the output must survive a prune"
        );
        assert!(
            !out.join("plain.md").exists(),
            "an untouched placeholder is not notes; that page is still pruned"
        );
        assert!(
            !out.join("migrated.md").exists(),
            "notes already migrated onto the renamed page leave nothing to keep"
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
