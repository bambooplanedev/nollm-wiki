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
                out.push_str(&format!(
                    "    +{} more named {}{place}; narrow with {owner}a path word\n",
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

/// What a `search` answers, which its line and JSON renderings share.
struct Answer {
    /// Names a page whose members were not indexed holds, with that page's
    /// path and member count (`Wiki::in_unindexed_members`).
    unindexed: Vec<(String, String, usize)>,
    /// Names defined only on test pages (`Wiki::in_tests_only`).
    tests_only: Vec<String>,
    /// Their test definitions (`Wiki::test_definitions`).
    test_hits: Vec<Hit>,
    /// The other names no item defines (`Wiki::not_found`).
    missing: Vec<String>,
    /// The hits are the ranked pages: no name of the query is defined.
    ranked: bool,
    hits: Vec<Hit>,
    /// A test page ranked first and `code` dropped it.
    tests_cut: bool,
}

/// A query of names alone that names something only tests define is
/// answered with those test definitions, and the rest of the query without
/// those names.
fn answer(
    w: &Wiki,
    q: &str,
    kind: Option<&KindFilter>,
    tag: Option<&str>,
    limit: Option<usize>,
) -> Answer {
    let missing = w.not_found(q, kind, tag);
    let unindexed = w.in_unindexed_members(&missing, kind, tag);
    let missing: Vec<String> = missing
        .into_iter()
        .filter(|n| !unindexed.iter().any(|(u, _, _)| u == n))
        .collect();
    let tests_only = w.in_tests_only(&missing, kind, tag);
    let test_hits = if tests_only.is_empty() {
        Vec::new()
    } else {
        w.test_definitions(&tests_only, tag, limit.unwrap_or(10))
    };
    let missing: Vec<String> = missing
        .into_iter()
        .filter(|n| !tests_only.contains(n))
        .collect();
    let (names, _) = Wiki::query_names(q);
    let rest = names
        .into_iter()
        .filter(|n| !tests_only.iter().any(|t| t == n))
        .collect::<Vec<_>>()
        .join(" ");
    let q = if tests_only.is_empty() { q } else { &rest };
    let (hits, tests_cut) = if !tests_only.is_empty() && rest.is_empty() {
        (Vec::new(), false)
    } else {
        w.search_answer(q, kind, tag, limit)
    };
    let ranked = !missing.is_empty() && missing.len() == Wiki::query_names(q).0.len();
    Answer {
        unindexed,
        tests_cut: tests_cut && tests_only.is_empty(),
        tests_only,
        test_hits,
        missing,
        ranked,
        hits,
    }
}

/// The default line answer to a `search`: for a query of names alone, a
/// line sending names only tests define to `tests`, with their test
/// definitions, then a line naming the names no item defines, saying the
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
    if !a.tests_only.is_empty() {
        out += &format!(
            "{}: in test pages only; {USE_TESTS}\n",
            a.tests_only.join(", ")
        );
        out += &search_lines(&a.test_hits);
    }
    if !a.missing.is_empty() {
        let names = a.missing.join(", ");
        out += &match (a.hits.is_empty() || !a.ranked, a.missing.len()) {
            (true, _) => format!("no symbol named {names}\n"),
            (false, 1) => format!("no symbol named {names}; the pages below match its words\n"),
            (false, _) => format!("no symbol named {names}; the pages below match their words\n"),
        };
    }
    if !a.hits.is_empty() || (a.missing.is_empty() && a.tests_only.is_empty()) {
        out += &search_lines(&a.hits);
    }
    if a.tests_cut {
        out += &format!("tests also match; {USE_TESTS}\n");
    }
    out
}

/// `search_text`'s answer as `search_json`: the test definitions, then the
/// hits. The first hit adds `no_symbol` and `tests_only` (name lists) and
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
    let mut hits = hit_values(a.test_hits.into_iter().chain(a.hits).collect());
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
        if !a.tests_only.is_empty() {
            first["tests_only"] = a.tests_only.into();
        }
        if a.tests_cut {
            first["tests_also_match"] = true.into();
        }
    }
    serde_json::Value::Array(hits).to_string()
}
