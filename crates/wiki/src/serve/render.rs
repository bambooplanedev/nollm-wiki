//! Output formatting for `search`/`docs` results: the JSON and plain-line renderings and the memory-note prefix.

use crate::model::KindFilter;
use crate::model::{clip, one_line, Origin, LINE_CHARS};
use crate::query::{Hit, Wiki};

/// `docs` hits as JSON: `{"hits": [{path, start_line, end_line, label}], "errors": [..]}`, plus `"more": n` when the limit (or an empty query's default cap) cut n labels and `"fallback": line` when a fallback answered.
pub fn doc_json(
    hits: &[crate::labels::DocHit],
    errors: &[String],
    more: usize,
    fallback: Option<&str>,
) -> String {
    let hits: Vec<_> = hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "path": h.path,
                "start_line": h.start,
                "end_line": h.end,
                "label": h.text,
            })
        })
        .collect();
    let mut out = serde_json::json!({"hits": hits, "errors": errors});
    if more > 0 {
        out["more"] = more.into();
    }
    if let Some(f) = fallback {
        out["fallback"] = f.into();
    }
    out.to_string()
}

/// Render search hits as the JSON array `search` returns with
/// `format: "json"`: `[{id, title, summary, updated, score, snippet, path, symbols}]`;
/// `updated` is present only when the page has a date. A memory note adds
/// `memory: true` and `origin` (`local` or `repo`), plus `author` when known
/// and `edited: true` when the file differs from its last commit.
pub fn search_json(hits: Vec<Hit>) -> String {
    serde_json::Value::Array(hit_values(hits)).to_string()
}

/// `search_json`'s objects, one per hit. A symbol past the per-name cap
/// adds `more: {count, pages, owned}` (`SymbolLine::hidden`).
fn hit_values(hits: Vec<Hit>) -> Vec<serde_json::Value> {
    hits.into_iter()
        .map(|h| {
            let symbols: Vec<_> = h
                .symbols
                .iter()
                .map(|s| {
                    let mut sym = serde_json::json!({
                        "path": s.path,
                        "start_line": s.start_line,
                        "end_line": s.end_line,
                        "name": s.name,
                        "signature": s.signature,
                        "doc": s.doc,
                    });
                    if let Some(m) = &s.hidden {
                        sym["more"] = serde_json::json!({
                            "count": m.count,
                            "pages": m.pages,
                            "owned": m.owned,
                        });
                        if m.of_owner {
                            sym["more"]["of_owner"] = true.into();
                        }
                    }
                    sym
                })
                .collect();
            let mut hit = serde_json::json!({
                "id": h.id,
                "title": h.title,
                "summary": h.summary,
                "score": h.score,
                "snippet": h.snippet,
                "path": h.path,
                "symbols": symbols,
            });
            if let Some(u) = h.updated {
                hit["updated"] = u.into();
            }
            if h.memory {
                hit["memory"] = true.into();
                match h.origin {
                    Origin::Local => hit["origin"] = "local".into(),
                    Origin::Repo {
                        author,
                        edited,
                        mine,
                    } => {
                        hit["origin"] = "repo".into();
                        if let Some(a) = author {
                            hit["author"] = a.into();
                        }
                        if edited {
                            hit["edited"] = true.into();
                        }
                        if mine {
                            hit["mine"] = true.into();
                        }
                    }
                }
            }
            if let Some(by) = h.superseded_by {
                hit["superseded_by"] = by.into();
            }
            hit
        })
        .collect()
}

/// `memory (local)`, `memory (repo, Jane Doe)`: how a note row opens in
/// `search` lines and in the hook.
#[must_use]
pub fn memory_prefix(o: &Origin) -> String {
    format!("memory ({})", o.label())
}

/// Render search hits as the default line output: `id  path  [date  ]title — summary`
/// per page, then one `  path:start-end  name  signature  — doc` line per
/// symbol (`SymbolLine::called_by` adds `  [called by N: callers]`, callers in backticks,
/// before the doc; a function with exactly one other definition elsewhere adds
/// `    also defined: path:start-end  name`; the last line shown for a name
/// past `DEFINITIONS_PER_NAME` adds `    +N more named <name> …`), or
/// the snippet (four-space indent) when the page has no symbols. A summary,
/// snippet or doc is cut at `LINE_CHARS`, as a hint line is.
/// A memory note renders as the hook's one line, `memory (origin): path — summary`
/// (the title when there is no summary; see [`memory_prefix`]). No hits render as one `no matches` line.
pub fn search_lines(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "no matches\n".to_string();
    }
    // Fields are one-line text; collapse any newline or tab they carry.
    let mut out = String::new();
    let shown: std::collections::HashSet<_> = hits
        .iter()
        .flat_map(|h| &h.symbols)
        .map(|s| (&s.path, s.start_line, s.end_line))
        .collect();
    for h in hits {
        if h.memory {
            let said = h.summary.as_deref().filter(|s| !s.trim().is_empty());
            out.push_str(&format!(
                "{}: {} — {}",
                memory_prefix(&h.origin),
                h.path,
                clip(said.unwrap_or(&h.title), LINE_CHARS)
            ));
            // Only a tag listing shows a superseded note.
            if let Some(by) = &h.superseded_by {
                out.push_str(&format!(" (superseded by {by})"));
            }
            out.push('\n');
            if let Some(sn) = &h.snippet {
                out.push_str(&format!("    {}\n", clip(sn, LINE_CHARS)));
            }
            continue;
        }
        out.push_str(&format!("{}  {}  ", h.id, h.path));
        if let Some(u) = &h.updated {
            out.push_str(&format!("{u}  "));
        }
        out.push_str(&one_line(&h.title));
        if let Some(s) = &h.summary {
            out.push_str(&format!(" — {}", clip(s, LINE_CHARS)));
        }
        if let Some(by) = &h.superseded_by {
            out.push_str(&format!(" (superseded by {by})"));
        }
        out.push('\n');
        for s in &h.symbols {
            out.push_str(&format!(
                "  {}:{}-{}  {}  {}",
                s.path,
                s.start_line,
                s.end_line,
                s.name,
                one_line(&s.signature)
            ));
            if let Some(n) = s.called_by {
                out.push_str(&format!("  [called by {n}: `callers`]"));
            }
            if let Some(d) = &s.doc {
                out.push_str(&format!("  — {}", clip(d, LINE_CHARS)));
            }
            out.push('\n');
            if let Some((p, a, b, name)) = &s.also {
                if !shown.contains(&(p, *a, *b)) {
                    out.push_str(&format!("    also defined: {p}:{a}-{b}  {name}\n"));
                }
            }
            if let Some(m) = &s.hidden {
                let place = match m.pages {
                    0 => " on the pages shown".to_string(),
                    1 => " on 1 other page".to_string(),
                    n => format!(" on {n} other pages"),
                };
                let owner = if m.owned {
                    format!("Class::{} or ", m.name)
                } else {
                    String::new()
                };
                let named = if m.of_owner { "owned by" } else { "named" };
                out.push_str(&format!(
                    "    +{} more {named} {}{place}; narrow with {owner}a path word\n",
                    m.count, m.name
                ));
            }
        }
        if let (true, Some(sn)) = (h.symbols.is_empty(), &h.snippet) {
            out.push_str(&format!("    {}\n", clip(sn, LINE_CHARS)));
        }
    }
    out
}

/// The line that sends a `code` search to `tests`.
const USE_TESTS: &str = "use kind \"tests\"";

/// The pages and kind a search of `other` (`KindFilter::other`) names:
/// `("test", "tests")` or `("code", "code")`.
fn kind_words(other: &KindFilter) -> (&'static str, &'static str) {
    match other {
        KindFilter::Tests => ("test", "tests"),
        _ => ("code", "code"),
    }
}

/// What a `search` answers, which its line and JSON renderings share.
struct Answer {
    /// Names a page whose members were not indexed holds, with that page's
    /// path and member count (`Wiki::in_unindexed_members`).
    unindexed: Vec<(String, String, usize)>,
    /// Names defined only in the other kind (`Wiki::in_other_kind`): on
    /// test pages for a `code` search, outside them for a `tests` one.
    other_only: Vec<String>,
    /// That kind (`KindFilter::other`).
    other: Option<KindFilter>,
    /// Their definitions there (`Wiki::definitions_in`).
    other_hits: Vec<Hit>,
    /// The other names no item defines (`Wiki::not_found`).
    missing: Vec<String>,
    /// The hits are the ranked pages: no name of the query is defined
    /// (under `tests`, the names only code defines count as not defined).
    ranked: bool,
    /// How many names of `other_only` the ranking kept: all of them under
    /// `tests`, or under `code` when one was typed `A.B` (a word, too).
    kept: usize,
    hits: Vec<Hit>,
    /// A test page ranked first and `code` dropped it.
    tests_cut: bool,
}

/// A query of names alone that names something only the other kind defines
/// (tests for `code`, code for `tests`) is answered with those definitions,
/// and the rest of the query without those names.
fn answer(
    w: &Wiki,
    q: &str,
    kind: Option<&KindFilter>,
    tag: Option<&str>,
    limit: Option<usize>,
) -> Answer {
    // Names are looked up as `A::B`; pages rank on the words as typed.
    let typed_q = q;
    let (q, typed) = match w.dotted_as_qualified(q, kind, tag) {
        Some((rewritten, typed)) => (std::borrow::Cow::Owned(rewritten), typed),
        None => (std::borrow::Cow::Borrowed(q), Vec::new()),
    };
    let q = q.as_ref();
    // A name is echoed as the user typed it (`Suite.Name`).
    let as_typed = |names: Vec<String>| -> Vec<String> {
        names
            .into_iter()
            .map(|n| {
                typed
                    .iter()
                    .find(|(r, _)| *r == n)
                    .map_or(n, |(_, t)| t.clone())
            })
            .collect()
    };
    let missing = w.not_found(q, kind, tag);
    let unindexed = w.in_unindexed_members(&missing, kind, tag);
    let missing: Vec<String> = missing
        .into_iter()
        .filter(|n| !unindexed.iter().any(|(u, _, _)| u == n))
        .collect();
    let other_only = w.in_other_kind(&missing, kind, tag);
    let other = kind.and_then(KindFilter::other);
    let other_hits = match &other {
        Some(o) if !other_only.is_empty() => {
            w.definitions_in(&other_only, o, tag, limit.unwrap_or(10))
        }
        _ => Vec::new(),
    };
    let missing: Vec<String> = missing
        .into_iter()
        .filter(|n| !other_only.contains(n))
        .collect();
    let (names, _) = Wiki::query_names(q);
    let names = names.into_iter().map(str::to_string).collect::<Vec<_>>();
    let rest: Vec<String> = names
        .iter()
        .filter(|n| !other_only.contains(n))
        .cloned()
        .collect();
    let rest_len = rest.len();
    let rest = as_typed(rest).join(" ");
    // Under `tests` a name only code defines still ranks the test pages
    // that mention it: finding those is why one searches `tests`. A word
    // typed `A.B` ranks as typed under any kind, as it did before it named
    // anything.
    let keep = matches!(other, Some(KindFilter::Code))
        || other_only.iter().any(|n| typed.iter().any(|(r, _)| r == n));
    let (q, name_count) = if other_only.is_empty() || keep {
        (typed_q, names.len())
    } else {
        (rest.as_str(), rest_len)
    };
    let (hits, tests_cut) = if !other_only.is_empty() && !keep && rest.is_empty() {
        (Vec::new(), false)
    } else {
        w.search_answer(q, kind, tag, limit)
    };
    let kept = if keep { other_only.len() } else { 0 };
    let ranked = !missing.is_empty() || kept > 0;
    let ranked = ranked && missing.len() + kept == name_count;
    Answer {
        unindexed,
        tests_cut: tests_cut && other_only.is_empty(),
        kept,
        other_only: as_typed(other_only),
        other,
        other_hits,
        missing: as_typed(missing),
        ranked,
        hits,
    }
}

/// The default line answer to a `search`: for a query of names alone, a
/// line sending names only the other kind defines to that kind (`code` to
/// `tests`, `tests` to `code`), with their definitions there, then a line naming the names no item defines, saying the
/// pages below match their words when they are the ranked pages; then
/// `search_lines` of the hits, and a last line when a test page ranked
/// first under `code`.
pub fn search_text(
    w: &Wiki,
    q: &str,
    kind: Option<&KindFilter>,
    tag: Option<&str>,
    limit: Option<usize>,
) -> String {
    let a = answer(w, q, kind, tag, limit);
    let mut out = String::new();
    for (name, path, n) in &a.unindexed {
        out += &format!(
            "{name}: members of {path} are not indexed ({n} there, more than {})\n",
            wiki_code::MEMBERS_PER_FILE
        );
    }
    if let (false, Some(other)) = (a.other_only.is_empty(), &a.other) {
        let (pages, kind) = kind_words(other);
        out += &format!(
            "{}: in {pages} pages only; use kind \"{kind}\"\n",
            a.other_only.join(", ")
        );
        out += &search_lines(&a.other_hits);
        if a.missing.is_empty() && a.ranked && !a.hits.is_empty() {
            let whose = if a.other_only.len() == 1 {
                "its"
            } else {
                "their"
            };
            // The pages below are of the kind searched, the other's other.
            let pages = if matches!(other, KindFilter::Code) {
                "test"
            } else {
                "code"
            };
            out += &format!("the {pages} pages below match {whose} words\n");
        }
    }
    if !a.missing.is_empty() {
        let names = a.missing.join(", ");
        out += &match (a.hits.is_empty() || !a.ranked, a.missing.len() + a.kept) {
            (true, _) => format!("no symbol named {names}\n"),
            (false, 1) => format!("no symbol named {names}; the pages below match its words\n"),
            (false, _) => format!("no symbol named {names}; the pages below match their words\n"),
        };
    }
    if !a.hits.is_empty() || (a.missing.is_empty() && a.other_only.is_empty()) {
        out += &search_lines(&a.hits);
    }
    if a.tests_cut {
        out += &format!("tests also match; {USE_TESTS}\n");
    }
    out
}

/// `search_text`'s answer as `search_json`: the other kind's definitions,
/// then the hits, the first ranked test page after them marked
/// `pages_match_words: true`. The first hit adds `no_symbol` and
/// `tests_only` or `code_only` (name lists) and
/// `tests_also_match: true` when the line answer has those lines; an answer
/// with no hit has nowhere to put them.
pub fn search_json_text(
    w: &Wiki,
    q: &str,
    kind: Option<&KindFilter>,
    tag: Option<&str>,
    limit: Option<usize>,
) -> String {
    let a = answer(w, q, kind, tag, limit);
    // The line answer's `the test (code) pages below match its words`.
    let marked = (!a.other_only.is_empty() && a.missing.is_empty() && a.ranked)
        .then_some(a.other_hits.len());
    let mut hits = hit_values(a.other_hits.into_iter().chain(a.hits).collect());
    if let Some(h) = marked.and_then(|n| hits.get_mut(n)) {
        h["pages_match_words"] = true.into();
    }
    if let Some(first) = hits.first_mut() {
        if !a.unindexed.is_empty() {
            let u: Vec<_> = a
                .unindexed
                .iter()
                .map(|(name, path, _)| serde_json::json!({"name": name, "path": path}))
                .collect();
            first["unindexed"] = u.into();
        }
        if !a.missing.is_empty() {
            first["no_symbol"] = a.missing.into();
        }
        if let (false, Some(other)) = (a.other_only.is_empty(), &a.other) {
            first[format!("{}_only", kind_words(other).1)] = a.other_only.into();
        }
        if a.tests_cut {
            first["tests_also_match"] = true.into();
        }
    }
    serde_json::Value::Array(hits).to_string()
}
