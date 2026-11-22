//! Output formatting for `search`/`docs` results: the JSON and plain-line renderings and the memory-note prefix.

use crate::model::{clip, one_line, Origin, LINE_CHARS};
use crate::query::Hit;

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
    let hits = hits
        .into_iter()
        .map(|h| {
            let symbols: Vec<_> = h
                .symbols
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "path": s.path,
                        "start_line": s.start_line,
                        "end_line": s.end_line,
                        "name": s.name,
                        "signature": s.signature,
                        "doc": s.doc,
                    })
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
        .collect();
    serde_json::Value::Array(hits).to_string()
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
                    0 => " on the pages above".to_string(),
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

/// A first line naming the `missing` names (`Wiki::not_found`) that no item
/// defines, then `search_lines` of `hits`. The line says the pages below, if
/// any, only mention the names. Names in `tests_only`
/// (`Wiki::in_tests_only`) get their own line instead, sending them to
/// `tests`; else `tests_cut` (`Wiki::search_answer`) adds a last line that
/// test pages also match.
pub fn search_lines_missing(
    hits: &[Hit],
    missing: &[String],
    tests_only: &[String],
    tests_cut: bool,
) -> String {
    let mut out = String::new();
    let plain: Vec<&str> = missing
        .iter()
        .filter(|n| !tests_only.contains(n))
        .map(String::as_str)
        .collect();
    if !plain.is_empty() {
        let names = plain.join(", ");
        out += &if hits.is_empty() {
            format!("no symbol named {names}\n")
        } else {
            format!("no symbol named {names}; the pages below only mention it\n")
        };
    }
    if !tests_only.is_empty() {
        out += &format!(
            "{}: in test pages only; {USE_TESTS}\n",
            tests_only.join(", ")
        );
    }
    if !hits.is_empty() || missing.is_empty() {
        out += &search_lines(hits);
    }
    if tests_cut && tests_only.is_empty() {
        out += &format!("tests also match; {USE_TESTS}\n");
    }
    out
}

#[cfg(test)]
mod missing_tests {
    use super::*;

    #[test]
    fn missing_names_come_first() {
        let out = search_lines_missing(&[], &["a_b".into(), "c_d".into()], &[], false);
        assert_eq!(out, "no symbol named a_b, c_d\n");
    }
}
