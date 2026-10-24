//! Page rendering: section parsing, preserved `## Notes`, wikilink rewriting, the render fingerprint, and atomic writes.

use crate::hash::{combine, to_hex};
use crate::model::{Edges, Entity, SourceKind};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
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
    (n >= 3).then_some((c, n))
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
pub fn parse_sections(text: &str) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let masked = mask_fenced_code(text);
    let caps: Vec<_> = SECTION.captures_iter(&masked).collect();
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
    let notes = parse_sections(&text)
        .get("Notes")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if notes == NOTES_PLACEHOLDER {
        String::new()
    } else {
        notes
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

    let mut index: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    for path in entries {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(meta) = parse_sections(&text).get("Metadata").cloned() else {
            continue;
        };
        for line in meta.lines() {
            if let Some(src) = line.trim().strip_prefix("- source: ") {
                index
                    .entry(src.trim().to_string())
                    .or_default()
                    .insert(path.clone());
                break;
            }
        }
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

pub fn render_page(
    entity: &Entity,
    edges: &Edges,
    entities: &BTreeMap<String, Entity>,
    preserved_notes: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", entity.name));
    out.push_str("<!-- generated by wiki compiler — do not edit compiler-owned sections -->\n\n");

    // Metadata (always present)
    out.push_str("## Metadata\n");
    out.push_str(&format!(
        "- created: {}\n",
        if entity.created.is_empty() {
            "unknown"
        } else {
            &entity.created
        }
    ));
    if !entity.updated.is_empty() {
        out.push_str(&format!("- updated: {}\n", entity.updated));
    }
    if !entity.aliases.is_empty() {
        out.push_str(&format!("- aliases: {}\n", entity.aliases.join(", ")));
    }
    if !entity.tags.is_empty() {
        out.push_str(&format!("- tags: {}\n", entity.tags.join(", ")));
    }
    out.push_str(&format!("- kind: {}\n", entity.kind.label()));
    out.push_str(&format!("- source: {}\n", entity.source_path));
    out.push_str(&format!(
        "- source_hash: {}\n\n",
        to_hex(&entity.content_hash)
    ));

    // Related (omit if empty)
    let related: Vec<String> = edges
        .outgoing
        .iter()
        .filter_map(|id| link_name(id, entities))
        .collect();
    if !related.is_empty() {
        out.push_str("## Related\n");
        for l in &related {
            out.push_str(&format!("- {l}\n"));
        }
        out.push('\n');
    }

    // Incoming edges, importers apart from the rest (each omitted if empty)
    for (heading, imported) in [("Imported by", true), ("Mentioned in", false)] {
        let refby: Vec<String> = edges
            .incoming
            .iter()
            .filter(|id| edges.imported_by.contains(*id) == imported)
            .filter_map(|id| link_name(id, entities))
            .collect();
        if !refby.is_empty() {
            out.push_str(&format!("## {heading}\n"));
            for l in &refby {
                out.push_str(&format!("- {l}\n"));
            }
            out.push('\n');
        }
    }

    // Code-only sections
    if let SourceKind::Code { .. } = entity.kind {
        if !entity.symbols.is_empty() {
            out.push_str("## Exports\n");
            for s in &entity.symbols {
                out.push_str(&format!("- `{s}`\n"));
            }
            out.push('\n');
        }
        if !entity.imports.is_empty() {
            out.push_str("## Imports\n");
            for i in &entity.imports {
                out.push_str(&format!("- {i}\n"));
            }
            out.push('\n');
        }
    }

    // Body (omit if empty)
    if !entity.body.trim().is_empty() {
        out.push_str("## Body\n");
        out.push_str(entity.body.trim());
        out.push_str("\n\n");
    }

    // Notes (human-owned, always present so there is a home for hand edits)
    out.push_str("## Notes\n");
    if preserved_notes.trim().is_empty() {
        out.push_str(NOTES_PLACEHOLDER);
        out.push('\n');
    } else {
        out.push_str(preserved_notes.trim());
        out.push('\n');
    }

    out
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
    let tmp = std::path::PathBuf::from(tmp);
    {
        let f = std::fs::File::create(&tmp)?;
        let mut w = std::io::BufWriter::new(f);
        w.write_all(content.as_bytes())?;
        w.flush()?;
    }
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
        // this project's own README does. Before the mask, the fenced `## Body`
        // won (BTreeMap insert is last-write-wins) and the real body was lost.
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
            "## Notes\nmy real notes\n\n## Body\n```\n## Notes\nexample notes\n```\n",
        )
        .unwrap();
        assert_eq!(read_notes_from(&path), "my real notes");
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
            aliases: vec![],
            tags: vec![],
            created: "2026-01-01".into(),
            updated: String::new(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
            defined: vec![],
            methods: vec![],
            items: vec![],
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
        let a = render_page(&ents["alpha"], &g.edges["alpha"], &ents, "");
        let b = render_page(&ents["beta"], &g.edges["beta"], &ents, "");
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
        let page = render_page(&ents["text"], &g.edges["text"], &ents, "");
        assert!(
            page.contains(
                "## Imported by\n- [[consumer|Consumer]]\n\n## Mentioned in\n- [[notes|Notes]]\n"
            ),
            "{page}"
        );
    }

    #[test]
    fn metadata_lists_tags_after_aliases() {
        let mut e = ent("alpha", "Alpha", "body");
        e.aliases = vec!["x".into()];
        e.tags = vec!["a".into(), "b/c".into()];
        let ents = map(vec![e]);
        let g = build_graph(&ents);
        let page = render_page(&ents["alpha"], &g.edges["alpha"], &ents, "");
        assert!(
            page.contains("- aliases: x\n- tags: a, b/c\n- kind:"),
            "{page}"
        );
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
        let page = render_page(&ents["alpha"], &g.edges["alpha"], &ents, "MY HAND NOTES");
        assert!(page.contains("MY HAND NOTES"));
        let sections = parse_sections(&page);
        assert_eq!(
            sections.get("Notes").map(|s| s.trim()),
            Some("MY HAND NOTES")
        );
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
    fn metadata_shows_updated_when_known() {
        let mut e = ent("alpha", "Alpha", "x");
        let render = |e: &Entity| {
            let ents = map(vec![e.clone()]);
            render_page(e, &build_graph(&ents).edges["alpha"], &ents, "")
        };
        assert!(!render(&e).contains("- updated:"));
        e.updated = "2026-10-23".into();
        assert!(render(&e).contains("- created: 2026-01-01\n- updated: 2026-10-23\n"));
    }
}
