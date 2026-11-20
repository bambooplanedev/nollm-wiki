//! Page-id naming: the collision ladder that makes every page id unique, and the reserved manifest names.

use crate::formats::code::{is_shell, module_stem, source_rank};
use crate::model::{basename, slugify, title_case, title_from_slug, Entity, SourceKind};
use std::collections::{BTreeMap, BTreeSet};

/// Base names (case-insensitive) reserved for manifest artifacts written
/// alongside pages: `index.md`/`index.json`, `llms.txt`, `AGENTS.md`, and
/// (with `--emit-json`) `graph.json`.
/// `items` is not reserved: a page with id `items` writes `items.md`, which
/// does not collide with `items.json`.
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
    if segs.last().map(|s| title_from_slug(s)).as_deref() == Some(base) {
        segs.pop();
    }
    let start = segs.len().saturating_sub(depth);
    let mut name = String::new();
    for seg in &segs[start..] {
        name.push_str(&title_from_slug(seg));
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
///
/// A code page with no extension is a shell script (`is_shell`) and takes
/// `sh`, as `deploy.sh` does beside `deploy.py`.
fn discriminator(entity: &Entity) -> String {
    let base = basename(&entity.source_path);
    let stem = base.split('.').next().unwrap_or(base);
    let code = matches!(entity.kind, SourceKind::Code { .. });
    if code && wiki_code::support::ext_of(base).is_empty() {
        return "sh".to_string();
    }
    let named_after_stem = code && module_stem(&entity.source_path) == stem;
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
                return slugify(&token);
            }
        }
    }
    slugify(stem)
}

/// The name an entity renders at a given rung of the ladder.
fn render_name(base: &str, source_path: &str, depth: usize, disc: &str, ordinal: usize) -> String {
    let mut name = qualify(base, source_path, depth);
    if !disc.is_empty() {
        name = format!("{name} {}", title_case(&disc.replace('_', " ")));
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
    fixed: &BTreeSet<String>,
) -> bool {
    // Every member of `group` currently shares this exact id (that is what
    // makes them a group) — vacate it once, for the whole group, rather than
    // per member: removing it again for a later member would just be
    // removing whatever a not-yet-processed sibling still has not replaced.
    // (A id taken by a fixed entity outside `extracted` stays claimed.)
    if !fixed.contains(&extracted[group[0]].id) {
        claimed.remove(&extracted[group[0]].id);
    }

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
            let candidate_id = slugify(&candidate);
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
pub(crate) fn disambiguate_ids(extracted: &mut [Entity]) {
    disambiguate_taken(extracted, &BTreeSet::new());
}

/// `disambiguate_ids` for entities that must also avoid `taken` ids, which
/// stay fixed: an entity whose id is taken is treated as colliding, never as
/// the winner, so it is renamed through the same ladder.
pub(crate) fn disambiguate_taken(extracted: &mut [Entity], taken: &BTreeSet<String>) {
    let base: Vec<String> = extracted.iter().map(|e| e.name.clone()).collect();
    let disc: Vec<String> = extracted.iter().map(discriminator).collect();
    let key: Vec<(u8, usize)> = extracted
        .iter()
        .map(|e| {
            let basename = basename(&e.source_path);
            (source_rank(&e.source_path), basename.matches('.').count())
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
        claimed.extend(taken.iter().cloned());

        let mut changed = false;
        for (id, group) in by_id
            .iter()
            .filter(|(id, g)| g.len() > 1 || taken.contains(*id))
        {
            let group = group.as_slice();
            let is_taken = taken.contains(id);
            if !force_free_ids {
                let first_parents = parent_segments(&extracted[group[0]].source_path);
                let same_dir = group
                    .iter()
                    .all(|&i| parent_segments(&extracted[i].source_path) == first_parents);
                // A non-shell page that strictly outranks shell pages keeps
                // its id: only the shell pages are qualified.
                let pinned = strict_winner(group, &key).filter(|&w| {
                    !is_shell(&extracted[w].source_path)
                        && group
                            .iter()
                            .all(|&i| i == w || is_shell(&extracted[i].source_path))
                });
                let can_deepen = group.iter().any(|&i| {
                    Some(i) != pinned && depth[i] < parent_segments(&extracted[i].source_path).len()
                });

                if !same_dir && can_deepen {
                    for &i in group {
                        if Some(i) != pinned
                            && depth[i] < parent_segments(&extracted[i].source_path).len()
                        {
                            depth[i] += 1;
                            changed = true;
                        }
                    }
                    continue;
                }

                let winner = if is_taken {
                    None
                } else {
                    strict_winner(group, &key)
                };
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
            if assign_free_ids(group, extracted, &state, &mut ordinal, &mut claimed, taken) {
                changed = true;
            }
        }

        if !changed {
            return;
        }

        for i in 0..extracted.len() {
            let d = if used_disc[i] { disc[i].as_str() } else { "" };
            let name = render_name(&base[i], &extracted[i].source_path, depth[i], d, ordinal[i]);
            extracted[i].id = slugify(&name);
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
pub(crate) fn remap_reserved_names(entities: BTreeMap<String, Entity>) -> BTreeMap<String, Entity> {
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
mod tests;
