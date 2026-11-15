//! Page rendering: section parsing, preserved `## Notes`, wikilink rewriting, the render fingerprint, and atomic writes.

use crate::hash::{combine, to_hex};
use crate::model::{Edges, Entity, SourceKind};
use rayon::prelude::*;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^##\s+(.+)$").unwrap());

/// Placeholder written into the `## Notes` section when an entity has no
/// hand-authored notes. `read_notes_from` treats it as "no notes" so a
/// fresh build and its recompiles fingerprint identically (see
/// `render_fingerprint`); without this, the first recompile after a fresh
/// build re-reads the placeholder, computes a different fingerprint, and
/// needlessly rewrites every page.
const NOTES_PLACEHOLDER: &str = "_(add your own notes here — preserved on recompile)_";

/// The `(char, run length)` of a fence delimiter — a line whose first
/// non-space run is three or more backticks or tildes.
fn fence_delim(trimmed: &str) -> Option<(char, usize)> {
    let c = trimmed.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = trimmed.chars().take_while(|x| *x == c).count();
    // CommonMark: a backtick fence's info string holds no backtick, so
    // "```x``` y" is inline code, not an opener.
    let inline = c == '`' && trimmed[n..].contains('`');
    (n >= 3 && !inline).then_some((c, n))
}

/// Overwrite every byte of `line` except its newline with an ASCII space.
/// Byte length is preserved, so offsets taken from a mask stay valid in the
/// original text; blanking whole ranges (never a fragment of a multi-byte
/// char) keeps the result valid UTF-8.
fn blank_into(out: &mut String, line: &str) {
    out.extend(line.bytes().map(|b| if b == b'\n' { '\n' } else { ' ' }));
}

/// `text` with fenced code blocks — delimiters included — blanked, byte for
/// byte. A rendered page carries its source's body verbatim, so a doc that
/// shows an example wiki page puts `## Body` and `[[slug|Name]]` inside a
/// fence; scanned raw, those examples masquerade as a real heading or link.
/// Callers scan the mask and slice the original.
pub(crate) fn mask_fenced_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut open: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let delim = fence_delim(trimmed);
        let was_open = open.is_some();
        match (open, delim) {
            (None, Some(d)) => open = Some(d),
            // A closing fence carries no info string: same char, at least as
            // long as the opener, and nothing else on the line.
            (Some((c, n)), Some((dc, dn)))
                if dc == c && dn >= n && trimmed.trim_end().chars().all(|x| x == c) =>
            {
                open = None;
            }
            _ => {}
        }
        if was_open || open.is_some() {
            blank_into(&mut out, line);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// `text` with inline code spans blanked, byte for byte. A run of N backticks
/// opens a span that closes at the next run of exactly N **on the same line**;
/// an unclosed run is literal text and is left alone. The single-line bound is
/// what keeps the mask honest on a code page, whose body is verbatim source:
/// one odd backtick in a comment would otherwise re-pair every span below it.
fn mask_inline_code(text: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let src = line.as_bytes();
        let mut buf = src.to_vec();
        let mut i = 0;
        while i < src.len() {
            if src[i] != b'`' {
                i += 1;
                continue;
            }
            let n = src[i..].iter().take_while(|b| **b == b'`').count();
            let mut j = i + n;
            let close = loop {
                if j >= src.len() {
                    break None;
                }
                if src[j] != b'`' {
                    j += 1;
                    continue;
                }
                let m = src[j..].iter().take_while(|b| **b == b'`').count();
                if m == n {
                    break Some(j);
                }
                j += m;
            };
            match close {
                Some(j) => {
                    for b in &mut buf[i..j + n] {
                        if *b != b'\n' {
                            *b = b' ';
                        }
                    }
                    i = j + n;
                }
                None => i += n,
            }
        }
        out.extend_from_slice(&buf);
    }
    String::from_utf8(out).expect("blanked whole ranges with ASCII spaces — still UTF-8")
}

/// `text` with both fenced blocks and inline code spans blanked. Byte offsets
/// are preserved throughout, so a caller may scan this and index `text`.
pub(crate) fn mask_code(text: &str) -> String {
    mask_inline_code(&mask_fenced_code(text))
}

/// Split a rendered page into `## Heading` -> body. Headings are matched
/// against a fenced-code mask, so an example page quoted inside a fence never
/// overwrites the real section of the same name; the bodies are sliced from
/// `text` itself and keep the fence verbatim.
///
/// `render_page` writes `## Body` and `## Notes` last, in that order, and
/// neither is compiler text: the body is the source verbatim and the notes are
/// hand-written. So a `## ` line inside them (a heredoc, a doc's own
/// subheading, a note's subheading) is content: `## Body` runs to the last
/// `## Notes`, and `## Notes` to the end of the page.
pub fn parse_sections(text: &str) -> BTreeMap<String, String> {
    split_headings(text, true)
}

/// `split_headings(text, false)` splits at every unfenced `## ` line, a doc
/// body's own subheadings included — what search scores as headings.
pub(crate) fn split_headings(text: &str, page: bool) -> BTreeMap<String, String> {
    let masked = mask_fenced_code(text);
    let mut caps: Vec<_> = SECTION.captures_iter(&masked).collect();
    if page {
        if let Some(notes) = caps.iter().rposition(|c| c[1].trim() == "Notes") {
            caps.truncate(notes + 1);
            if let Some(body) = caps.iter().position(|c| c[1].trim() == "Body") {
                caps.drain(body + 1..notes);
            }
        }
    }
    let mut sections = BTreeMap::new();
    for (i, cap) in caps.iter().enumerate() {
        let m = cap.get(0).expect("capture group 0 always present");
        let heading = cap[1].trim().to_string();
        let start = m.end();
        let end = if i + 1 < caps.len() {
            caps[i + 1]
                .get(0)
                .expect("capture group 0 always present")
                .start()
        } else {
            text.len()
        };
        sections.insert(heading, text[start..end].trim_matches('\n').to_string());
    }
    sections
}

/// `## Notes` from an already-written page, with the untouched placeholder
/// treated as empty so the render fingerprint is stable across recompiles.
pub(crate) fn read_notes_from(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let sections = parse_sections(&text);
    let notes = sections.get("Notes").map_or("", |s| s.trim());
    if notes == NOTES_PLACEHOLDER {
        String::new()
    } else {
        notes.to_string()
    }
}

/// Manifest artifacts that live in the output directory but are not pages.
/// `lint::load_compiled_pages` skips the same pair for the same reason.
const NON_PAGE_FILES: &[&str] = &["index.md", "AGENTS.md"];

/// Every already-written page's `source_path`, mapped to EVERY file declaring it.
///
/// The identity that survives a page rename is its source path, not its id, so
/// this is what lets `## Notes` follow a page whose id changed. Read through
/// `parse_sections`, never by scanning lines: a page renders its source
/// verbatim in `## Body`, so a doc that quotes an example page would otherwise
/// register that example's `- source:` as its own and inherit its notes.
///
/// Several pages declaring one source is a NORMAL state, not a half-written
/// directory: a page whose id changed leaves its old file behind, and that
/// orphan goes on declaring the live source until a prune removes it. So the
/// mapping is one-to-many and the choice among candidates belongs to the
/// caller, which alone knows the live page's own file name — see
/// `prior_page_for`. Sorted by construction (`BTreeMap`/`BTreeSet`), so the
/// candidate order is identical across runs, machines and thread counts.
pub(crate) fn page_source_index(output: &Path) -> BTreeMap<String, BTreeSet<PathBuf>> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(output) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !NON_PAGE_FILES.contains(&n))
            })
            .collect(),
        Err(_) => return BTreeMap::new(),
    };
    entries.sort();

    // Read and parse in parallel (the files are independent); the map is then
    // built sequentially, and a `BTreeMap`/`BTreeSet` is order-independent.
    let found: Vec<(String, PathBuf)> = entries
        .par_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            // Front matter (current) or a `## Metadata` bullet (pages from <= 0.6.7).
            let src = front_matter(&text, "source")
                .map(|v| serde_json::from_str::<String>(&v).unwrap_or(v))
                .or_else(|| {
                    parse_sections(&text)
                        .get("Metadata")?
                        .lines()
                        .find_map(|l| l.trim().strip_prefix("- source: ").map(str::to_string))
                })?;
            Some((src.trim().to_string(), path.clone()))
        })
        .collect();
    let mut index: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    for (src, path) in found {
        index.entry(src).or_default().insert(path);
    }
    index
}

/// Which of the page files declaring one source path a live page should read
/// its `## Notes` from — and, equivalently, which one this compile consumes.
///
/// **The entity's own `<id>.md` wins whenever it is a candidate.** A page that
/// was not renamed this run holds its notes in its own file, and an orphan
/// from an earlier id scheme goes on declaring the same source; without this
/// rule the orphan's lexicographically smaller name wins, the live page's
/// notes are read out of a dead file and destroyed, and the orphan's stale
/// notes are transplanted onto the live page in their place.
///
/// Only when the page's own file is absent — the genuine rename case, where
/// `<new_id>.md` does not exist yet — does the smallest candidate win; being
/// the smallest of a `BTreeSet` that keeps the choice deterministic.
pub(crate) fn prior_page_for(candidates: &BTreeSet<PathBuf>, own: &Path) -> Option<PathBuf> {
    if candidates.contains(own) {
        return Some(own.to_path_buf());
    }
    candidates.first().cloned()
}

/// Replace `|` in a display name so it cannot break the `[[target|display]]`
/// split. (`]]`/newlines in a name are pre-existing pathological cases, unchanged.)
fn sanitize_display(name: &str) -> String {
    name.replace('|', "/")
}

/// A wikilink whose target is the entity's slug (its `<id>.md` filename) and
/// whose display text is the entity name — resolves in Obsidian/Quartz and in
/// our own lint. Example: `[[test_main|Test Main]]`.
fn link_name(id: &str, entities: &BTreeMap<String, Entity>) -> Option<String> {
    entities
        .get(id)
        .map(|e| format!("[[{}|{}]]", e.id, sanitize_display(&e.name)))
}

/// The value of `key` in a leading `---` YAML front matter block, as written
/// (strings are JSON-quoted); `None` for a page without one (pages <= 0.6.7).
pub(crate) fn front_matter(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let block = &rest[..rest.find("\n---\n")?];
    block
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(": "))
        .map(|v| v.trim().to_string())
}

fn longest_backtick_run(s: &str) -> usize {
    s.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// Obsidian fix-ups of prose, never of fenced or inline code, so a reader that
/// resolves links by file name sees what `calque lint` sees:
/// - `[[x]]` whose `slugify(x)` is a page becomes `[[id]]` (anchor and
///   `|display` kept); unresolvable links stay as written;
/// - a relative markdown link to an indexed page becomes `[text](id.md)`
///   (`id_of_path`: project-relative source path -> page id);
/// - `#` that Obsidian would read as a tag (a digit first, not all digits) is
///   escaped as `\#`. The rule was fitted in the app: after whitespace or line
///   start, the tag runs to whitespace, ASCII punctuation other than `-_/`, or
///   U+2000-206F / U+2E00-2E7F (`#76→` is a tag, `#1–#9` is not); a trailing
///   `/` is dropped.
pub(crate) fn obsidian_fixups(
    text: &str,
    source_path: &str,
    is_page: &dyn Fn(&str) -> bool,
    id_of_path: &dyn Fn(&str) -> Option<String>,
) -> String {
    static WIKI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[\[([^\[\]|#\n]+)(#[^\[\]|\n]*)?(\|[^\[\]\n]*)?\]\]").unwrap()
    });
    static MD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[[^\[\]\n]*\]\(([^)\s]+)\)").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r##"#[0-9][^\s!"#$%&'()*+,.:;<=>?@\[\\\]^`{|}~\x{2000}-\x{206F}\x{2E00}-\x{2E7F}]*"##,
        )
        .unwrap()
    });
    let mask = mask_code(text);
    // (start, end, replacement); the three kinds never overlap.
    let mut subs: Vec<(usize, usize, String)> = Vec::new();
    for c in WIKI.captures_iter(&mask) {
        let t = text[c.get(1).unwrap().range()].trim();
        let id = crate::model::slugify(t);
        if t == id || !is_page(&id) || is_page(t) {
            continue;
        }
        let tail = |i| c.get(i).map_or("", |g| &text[g.range()]);
        let m = c.get(0).unwrap();
        subs.push((
            m.start(),
            m.end(),
            format!("[[{id}{}{}]]", tail(2), tail(3)),
        ));
    }
    for c in MD.captures_iter(&mask) {
        let g = c.get(1).unwrap();
        let target = &text[g.range()];
        let Some(id) = crate::graph::resolve_link(source_path, target).and_then(|p| id_of_path(&p))
        else {
            continue;
        };
        let anchor = target.find('#').map_or("", |i| &target[i..]);
        subs.push((g.start(), g.end(), format!("{id}.md{anchor}")));
    }
    for m in TAG.find_iter(&mask) {
        let tag = text[m.start() + 1..m.end()].trim_end_matches('/');
        let after_space = text[..m.start()]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if after_space && !tag.chars().all(|c| c.is_ascii_digit()) {
            subs.push((m.start(), m.start(), "\\".to_string()));
        }
    }
    subs.sort_by_key(|s| (s.0, s.1));
    let mut out = String::with_capacity(text.len() + 16);
    let mut at = 0;
    for (a, b, r) in subs {
        out.push_str(&text[at..a]);
        out.push_str(&r);
        at = b;
    }
    out.push_str(&text[at..]);
    out
}

/// Source path to page id, first id in id order wins; built once per compile.
pub fn path_index(entities: &BTreeMap<String, Entity>) -> BTreeMap<&str, &str> {
    let mut m = BTreeMap::new();
    for e in entities.values() {
        m.entry(e.source_path.as_str()).or_insert(e.id.as_str());
    }
    m
}

pub fn render_page(
    entity: &Entity,
    edges: &Edges,
    entities: &BTreeMap<String, Entity>,
    by_path: &BTreeMap<&str, &str>,
    preserved_notes: &str,
) -> String {
    let mut out = String::new();
    // YAML front matter (Obsidian properties); readers also take the old
    // `## Metadata` bullets, see `front_matter`.
    let js = |v: &str| serde_json::to_string(v).expect("string");
    let list = |v: &[String]| serde_json::to_string(v).expect("list");
    let mut head = String::from("---\n");
    if !entity.created.is_empty() {
        head.push_str(&format!("created: {}\n", entity.created));
    }
    if !entity.updated.is_empty() {
        head.push_str(&format!("updated: {}\n", entity.updated));
    }
    if !entity.aliases.is_empty() {
        head.push_str(&format!("aliases: {}\n", list(&entity.aliases)));
    }
    let kind_tag = if entity.memory {
        "wiki/memory"
    } else if matches!(entity.kind, SourceKind::Code { .. }) {
        "wiki/code"
    } else {
        "wiki/doc"
    };
    let mut tags = vec![kind_tag.to_string()];
    tags.extend(
        entity
            .tags
            .iter()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join("-")),
    );
    head.push_str(&format!("tags: {}\n", list(&tags)));
    head.push_str(&format!("kind: {}\n", entity.kind.label()));
    head.push_str(&format!("source: {}\n", js(&entity.source_path)));
    head.push_str(&format!(
        "source_hash: {}\n---\n",
        to_hex(&entity.content_hash)
    ));
    out.push_str(&head);
    out.push_str(&format!("# {}\n\n", entity.name));
    out.push_str("<!-- generated by calque — do not edit compiler-owned sections -->\n\n");

    // Each list section is omitted if empty.
    list_section(
        &mut out,
        "Related",
        edges
            .outgoing
            .iter()
            .filter_map(|id| link_name(id, entities)),
    );
    // Incoming edges, importers apart from the rest
    for (heading, imported) in [("Imported by", true), ("Mentioned in", false)] {
        list_section(
            &mut out,
            heading,
            edges
                .incoming
                .iter()
                .filter(|id| edges.imported_by.contains(*id) == imported)
                .filter_map(|id| link_name(id, entities)),
        );
    }

    // Code-only sections
    if let SourceKind::Code { .. } = entity.kind {
        list_section(
            &mut out,
            "Exports",
            entity.symbols.iter().map(|s| format!("`{s}`")),
        );
        list_section(&mut out, "Imports", entity.imports.iter().cloned());
    }

    // Body (omit if empty)
    if !entity.body.trim().is_empty() {
        out.push_str("## Body\n");
        let body = entity.body.trim();
        match &entity.kind {
            // Fenced, so code comments are not headings, `[[x]]` is not a link
            // and `<T>` is not HTML in a markdown reader.
            SourceKind::Code { lang } => {
                let fence = "`".repeat((longest_backtick_run(body) + 1).max(3));
                out.push_str(&format!("{fence}{lang}\n{body}\n{fence}"));
            }
            _ => out.push_str(&obsidian_fixups(
                body,
                &entity.source_path,
                &|id| entities.contains_key(id),
                &|p| by_path.get(p).map(ToString::to_string),
            )),
        }
        out.push_str("\n\n");
    }

    // Notes (human-owned, always present so there is a home for hand edits)
    out.push_str("## Notes\n");
    let notes = preserved_notes.trim();
    out.push_str(if notes.is_empty() {
        NOTES_PLACEHOLDER
    } else {
        notes
    });
    out.push('\n');

    out
}

/// `## heading` and one `- item` line each, then a blank line; nothing when `items` is empty.
fn list_section(out: &mut String, heading: &str, items: impl IntoIterator<Item = String>) {
    let mut items = items.into_iter().peekable();
    if items.peek().is_none() {
        return;
    }
    out.push_str(&format!("## {heading}\n"));
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
    out.push('\n');
}

pub(crate) fn render_fingerprint(
    entity: &Entity,
    edges: &Edges,
    entities: &BTreeMap<String, Entity>,
    preserved_notes: &str,
) -> [u8; 32] {
    let names_of = |ids: &BTreeSet<String>| -> Vec<String> {
        ids.iter()
            .filter_map(|id| entities.get(id).map(|e| e.name.clone()))
            .collect()
    };
    let out_names = names_of(&edges.outgoing);
    let in_names = names_of(&edges.incoming);
    let importer_names = names_of(&edges.imported_by);
    combine(&[
        entity.name.as_bytes(),
        entity.created.as_bytes(),
        entity.updated.as_bytes(),
        entity.kind.label().as_bytes(),
        entity.source_path.as_bytes(),
        entity.body.as_bytes(),
        entity.aliases.join(",").as_bytes(),
        entity.content_hash.as_slice(),
        entity.symbols.join(",").as_bytes(),
        entity.imports.join(",").as_bytes(),
        out_names.join(",").as_bytes(),
        in_names.join(",").as_bytes(),
        importer_names.join(",").as_bytes(),
        preserved_notes.trim().as_bytes(),
        entity.summary.as_deref().unwrap_or("").as_bytes(),
    ])
}

/// Write via a temp file + rename so an interrupted run can't leave a half-written page.
pub(crate) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A leftover at the temporary path may be a link to anywhere: remove it,
    // never write through it.
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        Ok(()) | Err(_) => {}
    }
    // Exclusive create: a link planted since the removal makes this fail.
    std::io::Write::write_all(
        &mut std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?,
        content.as_bytes(),
    )?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
