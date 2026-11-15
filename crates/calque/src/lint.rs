//! Wiki health checks (`calque lint`, MCP `lint` tool): broken wikilinks, orphan pages and pages too large for a default `neighbors` pack over a compiled output directory.

use crate::model::{slugify, LintReport};
use crate::query::Wiki;
use crate::rewrite::{mask_code, parse_sections};
use regex::Regex;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[(.+?)\]\]").unwrap());

/// Read all rendered pages (`<id>.md`) from a compiled output directory,
/// skipping the non-page files `index.md` and `AGENTS.md`. Used by the
/// `lint` CLI subcommand and the MCP server's `lint` tool.
pub fn load_compiled_pages(dir: &std::path::Path) -> std::io::Result<BTreeMap<String, String>> {
    let mut pages = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem != "index" && stem != "AGENTS" {
            pages.insert(stem.to_string(), std::fs::read_to_string(&path)?);
        }
    }
    Ok(pages)
}

/// The slug side of a wikilink capture: everything before the first `|`
/// (`[[target|display]]`). A bare `[[x]]` returns `x`.
fn link_target(inner: &str) -> &str {
    inner.split_once('|').map_or(inner, |(target, _)| target)
}

/// The human-readable side of a wikilink capture: everything after the last `|`
/// (or the whole capture if there is no `|`). Used for broken-link reports.
fn link_display(inner: &str) -> &str {
    inner.rsplit_once('|').map_or(inner, |(_, display)| display)
}

/// Whether a rendered page's front matter says `kind: code:<lang>` (or, for
/// pages compiled by 0.6.7 or earlier, its `## Metadata` says `- kind: code:<lang>`).
fn is_code_page(text: &str, sections: &BTreeMap<String, String>) -> bool {
    crate::rewrite::front_matter(text, "kind").is_some_and(|k| k.starts_with("code:"))
        || sections.get("Metadata").is_some_and(|m| {
            m.lines()
                .any(|l| l.trim_start().starts_with("- kind: code:"))
        })
}

/// `pages`: page id -> rendered markdown. Purely in-memory (no disk re-read).
pub fn lint(pages: &BTreeMap<String, String>) -> LintReport {
    let mut incoming: BTreeMap<String, usize> = pages.keys().map(|k| (k.clone(), 0)).collect();
    let mut broken_links = Vec::new();
    let mut large = Vec::new();

    for (id, text) in pages {
        let sections = parse_sections(text);
        // A pack separator is charged to every block, so a page over the
        // budget with it is degraded to a summary; code pages are exempt
        // (their outline is the intended view).
        let tokens = (text.chars().count() + Wiki::SEPARATOR.chars().count()) / 4;
        let is_code = is_code_page(text, &sections);
        if !is_code && tokens > crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS {
            large.push((id.clone(), tokens));
        }
        // Scan a code mask, not the raw page: a `[[slug|Name]]` written as a
        // syntax example in a fenced block or inline code is not a link. A code
        // page goes further: its `## Body` is verbatim source, where a
        // `[[...]]` is a string literal or a comment, so the whole section is
        // dropped before the scan. Text and markdown bodies are scanned — a
        // wikilink there is the author's, and a broken one is the point.
        let scan: Cow<str> = match sections.get("Body") {
            Some(body) if is_code && !body.is_empty() => text.replacen(body.as_str(), "", 1).into(),
            _ => text.into(),
        };
        for cap in LINK.captures_iter(&mask_code(&scan)) {
            let inner = &cap[1];
            if !pages.contains_key(&slugify(link_target(inner))) {
                broken_links.push((id.clone(), link_display(inner).to_string()));
            }
        }
        // Orphan counting uses ONLY the Related section (true outgoing edges).
        if let Some(related) = sections.get("Related") {
            for cap in LINK.captures_iter(related) {
                let slug = slugify(link_target(&cap[1]));
                if let Some(c) = incoming.get_mut(&slug) {
                    *c += 1;
                }
            }
        }
    }

    let orphans: Vec<String> = incoming
        .into_iter()
        .filter(|(_, c)| *c == 0)
        .map(|(id, _)| id)
        .collect();
    LintReport {
        total_pages: pages.len(),
        broken_links,
        orphans,
        large,
    }
}

#[cfg(test)]
mod tests;
