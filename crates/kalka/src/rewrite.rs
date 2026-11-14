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
/// resolves links by file name sees what `kalka lint` sees:
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
    out.push_str("<!-- generated by kalka — do not edit compiler-owned sections -->\n\n");

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
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::model::{Entity, SourceKind};
    use std::collections::BTreeMap;

    #[test]
    fn fenced_example_headings_do_not_overwrite_real_sections() {
        // A page whose body shows an example rendered wiki page — exactly what
        // this project's own README does. Unmasked, the fenced `## Body`
        // would win (BTreeMap insert is last-write-wins) and the real body be lost.
        let page = "## Body\nthe real body\n\n```\n## Body\nexample body\n```\n";
        let sections = parse_sections(page);
        assert_eq!(sections.len(), 1);
        let body = &sections["Body"];
        assert!(body.starts_with("the real body"), "got: {body:?}");
        // Sliced from the original, so the fenced example survives verbatim.
        assert!(body.contains("example body"), "got: {body:?}");
    }

    #[test]
    fn fenced_notes_do_not_masquerade_as_preserved_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.md");
        std::fs::write(
            &path,
            "## Body\n```\n## Notes\nexample notes\n```\n\n## Notes\nmy real notes\n",
        )
        .unwrap();
        assert_eq!(read_notes_from(&path), "my real notes");
    }

    #[test]
    fn inline_code_at_line_start_does_not_open_a_fence() {
        assert_eq!(mask_fenced_code("```x``` y\n## H\n"), "```x``` y\n## H\n");
    }

    #[test]
    fn mask_preserves_byte_offsets_through_multibyte_code() {
        // Offsets from the mask index back into the original, so a mask that
        // changed length would panic or slice mid-char.
        let page = "## Body\n`«naïve café»`\n\n```\nbody → example\n```\n\n## Notes\nx\n";
        assert_eq!(mask_code(page).len(), page.len());
        assert_eq!(mask_fenced_code(page).len(), page.len());
        let sections = parse_sections(page);
        assert_eq!(sections["Notes"], "x");
        assert!(sections["Body"].contains("«naïve café»"));
    }

    #[test]
    fn unclosed_backtick_run_is_literal_text() {
        let text = "a `b [[ghost]] c\n";
        assert_eq!(mask_code(text), text);
    }

    #[test]
    fn a_stray_backtick_does_not_re_pair_later_lines() {
        // A code page's body is verbatim source; odd backticks in comments are
        // routine. Each line pairs on its own, so line 1 cannot swallow line 3.
        let text = "// a lone ` tick\n[[real_link]]\n// `[[example]]` shown\n";
        let masked = mask_code(text);
        assert_eq!(masked.len(), text.len());
        assert!(masked.contains("[[real_link]]"), "got: {masked:?}");
        assert!(!masked.contains("[[example]]"), "got: {masked:?}");
    }

    #[test]
    fn tilde_fence_closes_only_on_its_own_char() {
        let text = "~~~\n```\n## Inside\n~~~\n\n## Outside\ny\n";
        let sections = parse_sections(text);
        assert_eq!(sections.keys().collect::<Vec<_>>(), vec!["Outside"]);
    }

    fn ent(id: &str, name: &str, body: &str) -> Entity {
        Entity {
            id: id.into(),
            name: name.into(),
            created: "2026-01-01".into(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            ..Entity::default()
        }
    }
    fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
        v.into_iter().map(|e| (e.id.clone(), e)).collect()
    }

    #[test]
    fn related_and_mentioned_in_rendered() {
        let ents = map(vec![
            ent("alpha", "Alpha", "mentions Beta"),
            ent("beta", "Beta", "nothing"),
        ]);
        let g = build_graph(&ents);
        let a = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        );
        let b = render_page(
            &ents["beta"],
            &g.edges["beta"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(a.contains("## Related") && a.contains("[[beta|Beta]]"));
        assert!(b.contains("## Mentioned in") && b.contains("[[alpha|Alpha]]"));
        assert!(a.contains("generated"));
    }

    #[test]
    fn importers_and_mentions_render_apart() {
        let mut text = ent("text", "Text", "");
        text.source_path = "pkg/text.py".into();
        text.kind = SourceKind::Code {
            lang: "python".into(),
        };
        let mut consumer = ent("consumer", "Consumer", "");
        consumer.source_path = "pkg/consumer.py".into();
        consumer.kind = text.kind.clone();
        consumer.imports = vec![".text".into()];
        let ents = map(vec![text, consumer, ent("notes", "Notes", "see `text.py`")]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["text"],
            &g.edges["text"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(
            page.contains(
                "## Imported by\n- [[consumer|Consumer]]\n\n## Mentioned in\n- [[notes|Notes]]\n"
            ),
            "{page}"
        );
    }

    #[test]
    fn front_matter_lists_kind_tag_then_tags_after_aliases() {
        let mut e = ent("alpha", "Alpha", "body");
        e.aliases = vec!["x".into()];
        e.tags = vec!["a".into(), "b/c".into(), "two words".into()];
        let ents = map(vec![e]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(
            page.starts_with(
                "---\ncreated: 2026-01-01\naliases: [\"x\"]\n\
                 tags: [\"wiki/doc\",\"a\",\"b/c\",\"two-words\"]\nkind: text\n\
                 source: \"alpha.txt\"\nsource_hash: "
            ),
            "{page}"
        );
        assert!(
            !page.contains("## Metadata") && !page.contains("title:"),
            "{page}"
        );
        assert!(page.contains("\n---\n# Alpha\n"), "{page}");
    }

    #[test]
    fn front_matter_kind_tag_follows_the_page_kind() {
        let tag = |f: &dyn Fn(&mut Entity)| {
            let mut e = ent("alpha", "Alpha", "body");
            f(&mut e);
            let ents = map(vec![e]);
            let g = build_graph(&ents);
            render_page(
                &ents["alpha"],
                &g.edges["alpha"],
                &ents,
                &path_index(&ents),
                "",
            )
        };
        assert!(tag(&|e| e.kind = SourceKind::Code {
            lang: "rust".into()
        })
        .contains("tags: [\"wiki/code\"]"));
        assert!(tag(&|e| e.memory = true).contains("tags: [\"wiki/memory\"]"));
    }

    #[test]
    fn code_body_is_fenced_with_its_language_and_a_long_enough_fence() {
        let mut e = ent("alpha", "Alpha", "fn a() {}\n```\nx\n```");
        e.kind = SourceKind::Code {
            lang: "rust".into(),
        };
        let ents = map(vec![e]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(
            page.contains("## Body\n````rust\nfn a() {}\n```\nx\n```\n````\n\n## Notes"),
            "{page}"
        );
    }

    #[test]
    fn page_source_index_reads_front_matter_and_old_metadata_alike() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("new.md"),
            "---\nkind: text\nsource: \"dir/a \\\"b\\\".md\"\n---\n# N\n\n## Notes\nhand\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("old.md"),
            "# O\n\n## Metadata\n- source: old.md\n\n## Notes\nhand\n",
        )
        .unwrap();
        let idx = page_source_index(dir.path());
        assert!(idx.contains_key("dir/a \"b\".md"), "{idx:?}");
        assert!(idx.contains_key("old.md"), "{idx:?}");
    }

    #[test]
    fn fingerprint_changes_with_content_hash() {
        let a = ent("alpha", "Alpha", "body");
        let mut b = a.clone();
        b.content_hash = [1u8; 32];
        let ents = map(vec![a.clone()]);
        let g = build_graph(&ents);
        let e = &g.edges["alpha"];
        assert_ne!(
            render_fingerprint(&a, e, &ents, ""),
            render_fingerprint(&b, e, &ents, "")
        );
    }

    #[test]
    fn sanitize_display_replaces_pipe() {
        assert_eq!(sanitize_display("A|B"), "A/B");
        assert_eq!(sanitize_display("Normal Name"), "Normal Name");
    }

    #[test]
    fn preserved_notes_survive() {
        let ents = map(vec![ent("alpha", "Alpha", "body")]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "MY HAND NOTES",
        );
        assert!(page.contains("MY HAND NOTES"));
        let sections = parse_sections(&page);
        assert_eq!(
            sections.get("Notes").map(|s| s.trim()),
            Some("MY HAND NOTES")
        );
    }

    #[test]
    fn body_and_notes_keep_their_own_heading_lines() {
        // A code body may hold `## ` lines (a heredoc, a doc string) and hand
        // notes may have subheadings: neither is a page section.
        let mut e = ent(
            "alpha",
            "Alpha",
            "x\n## Metadata\n- source: fake\n## Notes\ny",
        );
        e.kind = SourceKind::Code {
            lang: "bash".into(),
        };
        let ents = map(vec![e]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "mine\n## Todo\nmore",
        );
        let sections = parse_sections(&page);
        assert_eq!(
            sections["Body"],
            "```bash\nx\n## Metadata\n- source: fake\n## Notes\ny\n```"
        );
        assert_eq!(sections["Notes"], "mine\n## Todo\nmore");
        assert_eq!(sections.keys().collect::<Vec<_>>(), vec!["Body", "Notes"]);
    }

    #[test]
    fn fingerprint_changes_with_incoming_edges() {
        let ents1 = map(vec![
            ent("a", "Alpha", "nothing"),
            ent("b", "Beta", "nothing"),
        ]);
        let g1 = build_graph(&ents1);
        let f1 = render_fingerprint(&ents1["a"], &g1.edges["a"], &ents1, "");
        let ents2 = map(vec![
            ent("a", "Alpha", "nothing"),
            ent("b", "Beta", "mentions Alpha"),
        ]);
        let g2 = build_graph(&ents2);
        let f2 = render_fingerprint(&ents2["a"], &g2.edges["a"], &ents2, "");
        assert_ne!(f1, f2); // Alpha gained an incoming edge → must re-render
    }

    #[test]
    fn page_source_index_reads_metadata_and_ignores_fenced_examples() {
        let dir = tempfile::tempdir().unwrap();
        let real = "# Order Book\n\n## Metadata\n- source: src/orderbook.h\n\n## Body\nreal page\n";
        std::fs::write(dir.path().join("order_book.md"), real).unwrap();

        // A doc page whose BODY quotes an example page. The quoted `- source:`
        // must not register: it would steal notes from the real page later.
        let doc = "# Readme\n\n## Metadata\n- source: README.md\n\n## Body\n```\n## Metadata\n- source: src/orderbook.h\n```\n";
        std::fs::write(dir.path().join("readme.md"), doc).unwrap();

        // Manifests are not pages and must be skipped.
        std::fs::write(
            dir.path().join("index.md"),
            "## Metadata\n- source: nope.txt\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "## Metadata\n- source: nope2.txt\n",
        )
        .unwrap();

        let idx = page_source_index(dir.path());
        let names = |src: &str| -> Vec<String> {
            idx.get(src)
                .into_iter()
                .flatten()
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
                .collect()
        };

        assert_eq!(
            names("src/orderbook.h"),
            vec!["order_book.md".to_string()],
            "fenced example stole the mapping: {idx:?}"
        );
        assert_eq!(names("README.md"), vec!["readme.md".to_string()]);
        assert!(
            !idx.contains_key("nope.txt"),
            "index.md is a manifest, not a page"
        );
        assert!(
            !idx.contains_key("nope2.txt"),
            "AGENTS.md is a manifest, not a page"
        );
    }

    #[test]
    fn page_source_index_keeps_every_page_declaring_one_source() {
        // An orphan from an earlier id scheme and the live page both declare
        // `note.md`. Dropping either at index time is what let the orphan
        // hijack the live page's notes: the caller cannot prefer the live
        // page's own file if it never sees it.
        let dir = tempfile::tempdir().unwrap();
        for name in ["alpha_one.md", "alpha_two.md"] {
            std::fs::write(
                dir.path().join(name),
                "# A\n\n## Metadata\n- source: note.md\n",
            )
            .unwrap();
        }

        let idx = page_source_index(dir.path());
        let got: Vec<&str> = idx["note.md"]
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(got, vec!["alpha_one.md", "alpha_two.md"]);
    }

    #[test]
    fn page_source_index_prefers_front_matter_over_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("p.md"),
            "---\nsource: \"a.rs\"\n---\n# P\n\n## Metadata\n- source: b.rs\n",
        )
        .unwrap();

        let idx = page_source_index(dir.path());
        assert_eq!(idx.len(), 1, "{idx:?}");
        assert_eq!(idx["a.rs"], BTreeSet::from([dir.path().join("p.md")]));
    }

    #[test]
    fn page_source_index_keeps_every_front_matter_page_declaring_one_source() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["one.md", "two.md"] {
            std::fs::write(dir.path().join(name), "---\nsource: \"a.rs\"\n---\n# A\n").unwrap();
        }

        let idx = page_source_index(dir.path());
        assert_eq!(idx.len(), 1, "{idx:?}");
        assert_eq!(
            idx["a.rs"],
            BTreeSet::from([dir.path().join("one.md"), dir.path().join("two.md")])
        );
    }

    #[test]
    fn page_source_index_skips_unreadable_pages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.md"), [0xffu8, 0xfe]).unwrap();
        std::fs::write(
            dir.path().join("good.md"),
            "---\nsource: \"a.rs\"\n---\n# G\n",
        )
        .unwrap();

        let idx = page_source_index(dir.path());
        assert_eq!(idx.len(), 1, "{idx:?}");
        assert_eq!(idx["a.rs"], BTreeSet::from([dir.path().join("good.md")]));
    }

    #[test]
    fn prior_page_for_prefers_the_page_s_own_file_over_a_smaller_orphan() {
        let orphan = PathBuf::from("/out/alpha_one.md");
        let own = PathBuf::from("/out/alpha_two.md");
        let both: BTreeSet<PathBuf> = [orphan.clone(), own.clone()].into_iter().collect();

        // Own file present: it wins, even though the orphan sorts first.
        assert_eq!(prior_page_for(&both, &own), Some(own.clone()));
        // Own file absent — a genuine rename: the smallest candidate wins,
        // deterministically.
        let renamed = PathBuf::from("/out/alpha_three.md");
        assert_eq!(prior_page_for(&both, &renamed), Some(orphan));
        // Nothing declares this source: the caller falls back to its own file.
        assert_eq!(prior_page_for(&BTreeSet::new(), &own), None);
    }

    #[test]
    fn front_matter_shows_updated_when_known() {
        let mut e = ent("alpha", "Alpha", "x");
        let render = |e: &Entity| {
            let ents = map(vec![e.clone()]);
            render_page(
                e,
                &build_graph(&ents).edges["alpha"],
                &ents,
                &path_index(&ents),
                "",
            )
        };
        assert!(!render(&e).contains("updated:"));
        e.updated = "2026-10-23".into();
        assert!(render(&e).contains("created: 2026-01-01\nupdated: 2026-10-23\n"));
        e.created.clear();
        assert!(!render(&e).contains("created:"));
    }

    fn fix(text: &str) -> String {
        let ids = ["alpha", "wiki_rig"];
        obsidian_fixups(text, "docs/a.md", &|id| ids.contains(&id), &|p| match p {
            "README.md" => Some("proj".to_string()),
            "docs/b.md" => Some("beta".to_string()),
            _ => None,
        })
    }

    #[test]
    fn wikilink_to_a_slug_resolved_page_is_written_as_its_id() {
        assert_eq!(
            fix("see [[wiki-rig]] and [[Wiki Rig|rig]]"),
            "see [[wiki_rig]] and [[wiki_rig|rig]]"
        );
        assert_eq!(fix("[[wiki-rig#Part]]"), "[[wiki_rig#Part]]");
        assert_eq!(fix("[[alpha]] [[nope-x]]"), "[[alpha]] [[nope-x]]");
    }

    #[test]
    fn relative_markdown_links_to_pages_become_page_files() {
        assert_eq!(
            fix("[r](../README.md) [b](b.md#x) [l](../LICENSE) [e](https://x.io/a.md)"),
            "[r](proj.md) [b](beta.md#x) [l](../LICENSE) [e](https://x.io/a.md)"
        );
    }

    #[test]
    fn hash_is_escaped_only_where_obsidian_reads_a_tag() {
        assert_eq!(
            fix("#76\u{2192} x #1\u{2013}#9 #43/ (#12-) PR #76"),
            "\\#76\u{2192} x #1\u{2013}#9 #43/ (#12-) PR #76"
        );
        assert_eq!(
            fix("a #37-x #57-59 #12 #abc"),
            "a \\#37-x \\#57-59 #12 #abc"
        );
    }

    #[test]
    fn fixups_skip_fenced_and_inline_code() {
        let t = "`[[wiki-rig]] #76-x [r](../README.md)`\n```\n[[wiki-rig]] #76-x\n```\n";
        assert_eq!(fix(t), t);
    }

    #[test]
    fn render_page_applies_fixups_to_text_bodies_only() {
        let mut a = ent("alpha", "Alpha", "[[wiki-rig]] [t](docs/beta.txt) #5-x");
        a.source_path = "a.txt".into();
        let mut b = ent("wiki_rig", "Rig", "x");
        b.source_path = "docs/beta.txt".into();
        let mut c = ent("code", "Code", "// [[wiki-rig]]");
        c.kind = SourceKind::Code {
            lang: "rust".into(),
        };
        let ents = map(vec![a, b, c]);
        let g = build_graph(&ents);
        let page = render_page(
            &ents["alpha"],
            &g.edges["alpha"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(
            page.contains("## Body\n[[wiki_rig]] [t](wiki_rig.md) \\#5-x\n"),
            "{page}"
        );
        let page = render_page(
            &ents["code"],
            &g.edges["code"],
            &ents,
            &path_index(&ents),
            "",
        );
        assert!(page.contains("// [[wiki-rig]]"), "{page}");
    }
}
