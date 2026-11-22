//! Resolving a page argument and building context packs around a page.

use super::{
    norm_path, unknown_page, ContextPack, Entry, PackBudget, SymbolLine, Wiki, OVER_BUDGET_NOTE,
};
use crate::model::{
    clip, is_test_path, CodeItem, CodeItemKind, KindFilter, ITEM_DOC_TEXT_MAX_CHARS,
};
use std::collections::{BTreeSet, HashMap};

impl Wiki {
    /// The symbol line of item `i` on the page at `path`.
    pub(super) fn symbol_line(&self, path: &str, i: &CodeItem) -> SymbolLine {
        SymbolLine {
            path: path.to_string(),
            start_line: i.start_line,
            end_line: i.end_line,
            name: i.qualified_name(),
            signature: i.signature.clone(),
            doc: i.doc.clone(),
            test: i.test,
            nested: i.nested,
            also: self.same_name(i, path),
            called_by: None,
            hidden: None,
        }
    }

    /// The other definition of a function's name, when there is exactly one:
    /// not test code, not in the same file, not a header declaration paired
    /// with a definition, not the item's own other half. A name with more
    /// copies (`new`, `main`) gets none, since a list would be noise.
    fn same_name(&self, item: &CodeItem, path: &str) -> Option<(String, usize, usize, String)> {
        if item.test || item.nested || !matches!(item.kind, CodeItemKind::Fn | CodeItemKind::Method)
        {
            return None;
        }
        let index = self.by_name.get_or_init(|| {
            let mut m: HashMap<String, Vec<(String, usize)>> = HashMap::new();
            for (id, items) in &self.items {
                for (k, i) in items.iter().enumerate() {
                    if matches!(i.kind, CodeItemKind::Fn | CodeItemKind::Method)
                        && !i.test
                        && !i.nested
                    {
                        m.entry(i.name.clone()).or_default().push((id.clone(), k));
                    }
                }
            }
            m
        });
        let mut others = index.get(&item.name)?.iter().filter_map(|(id, k)| {
            let e = self.entries.get(id)?;
            let i = &self.items[id][*k];
            let header = [".h", ".hpp", ".hh", ".hxx"]
                .iter()
                .any(|x| e.path.ends_with(x));
            let span = format!("{}:{}-{}", e.path, i.start_line, i.end_line);
            let own_half = item.twin.as_deref() == Some(span.as_str());
            (e.path != path && !(header && i.twin.is_some()) && !own_half)
                .then(|| (e.path.clone(), i.start_line, i.end_line, i.qualified_name()))
        });
        let one = others.next()?;
        others.next().is_none().then_some(one)
    }

    /// Ids within `depth` hops of `id` along either edge direction,
    /// including `id` itself.
    fn bfs_neighborhood(&self, id: &str, depth: usize) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        seen.insert(id.to_string());
        let mut frontier = vec![id.to_string()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for nid in &frontier {
                if let Some(e) = self.entries.get(nid) {
                    for n in e.neighbors_out.iter().chain(e.neighbors_in.iter()) {
                        if seen.insert(n.clone()) {
                            next.push(n.clone());
                        }
                    }
                }
            }
            frontier = next;
        }
        seen
    }

    /// Between the blocks of a context pack.
    pub(crate) const SEPARATOR: &str = "\n\n---\n\n";

    /// Pagerank of `id`, or 0 for an id the index does not know.
    fn pagerank_of(&self, id: &str) -> f64 {
        self.entries.get(id).map_or(0.0, |e| e.pagerank)
    }

    /// `resolve_id` with the unknown-page error text callers show.
    pub fn resolve(&self, arg: &str) -> Result<String, String> {
        self.resolve_near(arg).map_err(|(near, by_word)| {
            self.skipped_path(arg).unwrap_or_else(|| {
                let defined = if by_word {
                    self.defined_as_stem(arg)
                } else {
                    vec![]
                };
                unknown_page(arg, &near, &defined)
            })
        })
    }

    /// Up to 3 `path:start-end Owner::name` places of items named like the
    /// file stem of `arg` (`app/thumbnail.py` gives `thumbnail`), one per
    /// file: non-test pages first, then types and functions before values.
    /// A nested value (a C/C++ field) only when nothing else has the name.
    fn defined_as_stem(&self, arg: &str) -> Vec<String> {
        let norm = norm_path(arg);
        let file = norm.rsplit('/').next().unwrap_or(&norm);
        let stem = file.split('.').next().unwrap_or(file);
        if stem.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(bool, &str, &CodeItem)> = self
            .code_items()
            .flat_map(|(path, items)| items.iter().map(move |i| (path, i)))
            .filter(|(_, i)| {
                i.name == stem && !matches!(i.kind, CodeItemKind::Module | CodeItemKind::TopLevel)
            })
            .map(|(path, i)| (is_test_path(path), path, i))
            .collect();
        if found.iter().any(|(_, _, i)| !i.nested) {
            found.retain(|(_, _, i)| !i.nested);
        }
        found.sort_by_key(|(test, path, i)| {
            let value = i.kind == CodeItemKind::Value;
            (*test, value, *path, i.start_line)
        });
        let mut files = std::collections::HashSet::new();
        found
            .into_iter()
            .filter(|(_, path, _)| files.insert(*path))
            .take(3)
            .map(|(_, path, i)| format!("{} {}", i.span(path), i.qualified_name()))
            .collect()
    }

    /// The error for a relative path under a folder the compile prunes
    /// (`walk::pruned_dir`, outermost first), or `None`.
    fn skipped_path(&self, arg: &str) -> Option<String> {
        use std::path::{Component, Path};
        let norm = norm_path(arg);
        let comps: Vec<&str> = Path::new(&norm)
            .components()
            .map(|c| match c {
                Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let dirs = &comps[..comps.len().saturating_sub(1)];
        let at = dirs.iter().position(|d| crate::walk::pruned_dir(d))?;
        let name = dirs[at];
        // The compile input at or below `name`: the path names a page above
        // the input, not a pruned folder inside it.
        if self.root.is_empty() || dirs[at..].contains(&self.root.as_str()) {
            return None;
        }
        // A page under a folder of that name: it was compiled (`--no-ignore`,
        // or a hidden folder an ignore file whitelists).
        let compiled = |p: &str| p.rsplit('/').skip(1).any(|d| d == name);
        if self.entries.values().any(|e| compiled(&e.path)) {
            return None;
        }
        let what = if name.starts_with('.') {
            "hidden"
        } else {
            "skipped"
        };
        let mut text = format!(
            "{arg:?} is under a {what} `{name}` folder, which the wiki does not compile; read it with your file tool"
        );
        if norm.to_lowercase().ends_with(".md") {
            text +=
                &format!(", or pass \"{norm}\" in search's `docs` for its headings' line ranges");
        }
        Some(text)
    }

    /// Resolves what a caller typed to a page id: an exact id, a title
    /// (ignoring case), the slug of `arg`, then `arg` as a path (`\` read as
    /// `/`, one leading `./` dropped): the page whose source path is it, the
    /// one page whose path ends with `/arg`, or the page with the longest path
    /// that `arg` ends with as `/{root}/{path}` (an absolute path under the
    /// compiled directory; none when the wiki has no `root`). Otherwise up to
    /// 5 candidate ids, sorted: the several pages that suffix matched, else
    /// ids sharing a lowercase word with `arg`.
    pub fn resolve_id(&self, arg: &str) -> Result<String, Vec<String>> {
        self.resolve_near(arg).map_err(|(near, _)| near)
    }

    /// `resolve_id`; the error also says whether its candidates came from the
    /// shared-word fallback (or are none), not from a path or file name.
    fn resolve_near(&self, arg: &str) -> Result<String, (Vec<String>, bool)> {
        if self.entries.contains_key(arg) {
            return Ok(arg.to_string());
        }
        let by = |f: &dyn Fn(&Entry) -> bool| {
            let mut m = self.entries.values().filter(|e| f(e));
            match (m.next(), m.next()) {
                (Some(e), None) => Some(e.id.clone()),
                _ => None,
            }
        };
        if let Some(id) = by(&|e| e.title.to_lowercase() == arg.to_lowercase()) {
            return Ok(id);
        }
        let slug = crate::model::slugify(arg);
        if self.entries.contains_key(&slug) {
            return Ok(slug);
        }
        let norm = norm_path(arg);
        let norm = norm.as_str();
        if let Some(id) = by(&|e| e.path == norm) {
            return Ok(id);
        }
        let suffix = format!("/{norm}");
        let by_path: Vec<String> = self
            .entries
            .values()
            .filter(|e| e.path.ends_with(&suffix))
            .map(|e| e.id.clone())
            .collect();
        if let [id] = by_path.as_slice() {
            return Ok(id.clone());
        }
        if !by_path.is_empty() {
            return Err((by_path.into_iter().take(5).collect(), false));
        }
        // A longer path such as an absolute one, `…/{root}/{path}`: the
        // longest such page path. Without a root (an older wiki) nothing is.
        if let Some(e) = self
            .entries
            .values()
            .filter(|e| !self.root.is_empty() && !e.path.is_empty())
            .filter(|e| norm.ends_with(&format!("/{}/{}", self.root, e.path)))
            .max_by_key(|e| e.path.len())
        {
            return Ok(e.id.clone());
        }
        // The same file name elsewhere first: `src/x/venue_caps.rs` for a
        // missing `src/venue_caps.rs`, not every page with `venue` in its
        // title (on the bench, five plans).
        let name = norm.rsplit('/').next().unwrap_or(norm);
        let same_name: Vec<String> = self
            .entries
            .values()
            .filter(|e| e.path.rsplit('/').next() == Some(name))
            .map(|e| e.id.clone())
            .take(5)
            .collect();
        if !same_name.is_empty() {
            return Err((same_name, false));
        }
        let words = |s: &str| -> BTreeSet<String> {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_lowercase)
                .collect()
        };
        let want = words(arg);
        // BTreeMap iteration is already sorted by id; test pages go last.
        let mut near: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| !want.is_disjoint(&words(&e.id)) || !want.is_disjoint(&words(&e.title)))
            .collect();
        near.sort_by_key(|e| is_test_path(&e.path));
        Err((
            near.into_iter().take(5).map(|e| e.id.clone()).collect(),
            true,
        ))
    }

    /// Whether page `id` is a code page.
    pub(super) fn is_code(&self, id: &str) -> bool {
        self.entries
            .get(id)
            .is_some_and(|e| e.is_kind(&KindFilter::Code))
    }

    /// BFS to `depth` over `neighbors_out+neighbors_in`, then build a budgeted
    /// context pack: target first, neighbors ordered ascending by pagerank
    /// (highest-centrality lands last — "lost in the middle").
    ///
    /// `max_tokens` is a hard ceiling on the estimated size of the returned
    /// pack text (`manifest::token_estimate`, chars/4): every block is
    /// charged at the size of the text actually emitted. A code target is
    /// its outline, cut to the budget. Another target is its full page;
    /// when that alone would blow the ceiling, it degrades to a summary
    /// block (title + summary + a pointer at the `page` tool with
    /// `full: true`) so the neighborhood still fits. The one floor exception: the outline's
    /// header or the degraded block is always emitted, even when a
    /// pathologically small budget cannot contain it — a neighbors call
    /// always says something about the target.
    ///
    /// Neighbor selection walks the target's import neighbors (either
    /// direction) first, then the rest, each group in descending centrality,
    /// and keeps whichever fit the remaining budget (skip, not break), so
    /// the kept set is the highest-centrality set the greedy can admit —
    /// deliberately NOT a maximum-cardinality fill. A heavier high-
    /// centrality neighbor beats several lighter lower-centrality ones; see
    /// `full_neighbors_max_tokens_prefers_centrality_over_packing`.
    /// An import neighbor is its whole outline when that fits, else a
    /// summary block like every other neighbor. With `full_neighbors`,
    /// every neighbor is its whole outline (code) or full page (other).
    pub fn neighbors(&self, id: &str, depth: usize, budget: &PackBudget) -> Option<ContextPack> {
        let target = self.entries.get(id)?;

        let seen = self.bfs_neighborhood(id, depth);
        let imports: BTreeSet<&str> = target
            .imports_out
            .iter()
            .chain(&target.imports_in)
            .map(String::as_str)
            .collect();

        // Candidates (excluding target, of `kind` when given): import
        // neighbors first, then descending by pagerank, so every budget
        // below always considers them, and the highest-centrality neighbor,
        // first.
        let mut candidates: Vec<String> = seen
            .iter()
            .filter(|n| *n != id)
            .filter(|n| {
                budget
                    .kind
                    .as_ref()
                    .is_none_or(|k| self.entries.get(*n).is_some_and(|e| e.is_kind(k)))
            })
            .cloned()
            .collect();
        candidates.sort_by(|a, b| {
            imports
                .contains(b.as_str())
                .cmp(&imports.contains(a.as_str()))
                .then_with(|| self.pagerank_of(b).total_cmp(&self.pagerank_of(a)))
                .then_with(|| a.cmp(b))
        });

        // Apply max_nodes: keep target + the highest-centrality neighbors
        // (the head of the descending list); the lowest-centrality tail is
        // dropped first.
        if let Some(max) = budget.max_nodes {
            let keep = max.saturating_sub(1);
            candidates.truncate(keep);
        }

        // Budget math tracks emitted *chars* and converts with the same
        // chars/4 rule as `manifest::token_estimate` — summing per-block
        // token estimates would under-count the concatenation by up to one
        // token per block (floor division) and break the ceiling. The
        // integration test `max_tokens_is_a_hard_ceiling_on_pack_size`
        // pins the two implementations together.
        let fits = |chars: usize| match budget.max_tokens {
            Some(b) => chars / 4 <= b,
            None => true,
        };

        // Target block: a code page's outline, cut so that it and the
        // separator fit (the separator's 7 chars cost at most 2 tokens on
        // top of the outline's own count). Another page in full, degraded
        // to a summary block when it (plus separator) alone would blow the
        // ceiling.
        let full_target = self
            .page(id)
            .unwrap_or_else(|| format!("# {}\n", target.title));
        let sep_chars = Self::SEPARATOR.chars().count();
        let target_block = if self.is_code(id) {
            self.outline(id, budget.max_tokens.map(|m| m.saturating_sub(2)))?
        } else if fits(full_target.chars().count() + sep_chars) {
            full_target
        } else {
            format!(
                "# {} ({id})\n\n{}\n\n_body (~{} tokens) {OVER_BUDGET_NOTE}; call page with full: true for the whole page_\n",
                target.title,
                summary_line(target.summary.as_deref()),
                target.token_estimate,
            )
        };
        // Floor: the target block is charged but never dropped.
        let mut chars_used = target_block.chars().count() + sep_chars;

        // Admit neighbors in candidate order, each charged at the size of
        // the block actually emitted (outline, summary line or full page).
        let mut kept: Vec<(String, String)> = Vec::new();
        for nid in candidates {
            let Some(e) = self.entries.get(&nid) else {
                continue;
            };
            let summary = || {
                format!(
                    "## {} ({})\n{}\n\n",
                    e.title,
                    nid,
                    summary_line(e.summary.as_deref())
                )
            };
            let outline = || self.outline(&nid, None).map(|o| o + "\n");
            let block = if budget.full_neighbors {
                if self.is_code(&nid) {
                    outline()
                } else {
                    self.page(&nid)
                }
                .unwrap_or_default()
            } else if imports.contains(nid.as_str()) {
                outline()
                    .filter(|o| fits(chars_used + o.chars().count()))
                    .unwrap_or_else(summary)
            } else {
                summary()
            };
            let cost = block.chars().count();
            if !fits(chars_used + cost) {
                continue;
            }
            chars_used += cost;
            kept.push((nid, block));
        }

        // Emit ascending by pagerank so the highest-centrality neighbor
        // lands last ("lost in the middle" placement).
        kept.sort_by(|(a, _), (b, _)| {
            self.pagerank_of(a)
                .total_cmp(&self.pagerank_of(b))
                .then_with(|| a.cmp(b))
        });

        let mut text = String::new();
        let mut included = vec![id.to_string()];
        text.push_str(&target_block);
        text.push_str(Self::SEPARATOR);
        for (nid, block) in kept {
            text.push_str(&block);
            included.push(nid);
        }

        Some(ContextPack { text, included })
    }
}

/// A page summary for a pack block: clipped, `(no summary)` when absent.
fn summary_line(summary: Option<&str>) -> String {
    summary.map_or_else(
        || "(no summary)".to_string(),
        |s| clip(s, ITEM_DOC_TEXT_MAX_CHARS),
    )
}
