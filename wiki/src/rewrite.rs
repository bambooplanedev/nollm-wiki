use crate::hash::{combine, to_hex};
use crate::model::{Edges, Entity, SourceKind};
use regex::Regex;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

static SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^##\s+(.+)$").unwrap());

/// Placeholder written into the `## Notes` section when an entity has no
/// hand-authored notes. `read_preserved_notes` treats it as "no notes" so a
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
fn mask_fenced_code(text: &str) -> String {
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
                open = None
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

pub fn read_preserved_notes(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let notes = parse_sections(&text)
                .get("Notes")
                .cloned()
                .unwrap_or_default()
                .trim()
                .to_string();
            // The unedited placeholder is not real notes — treat it as empty so
            // the render fingerprint is stable across recompiles.
            if notes == NOTES_PLACEHOLDER {
                String::new()
            } else {
                notes
            }
        }
        Err(_) => String::new(),
    }
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
    if !entity.aliases.is_empty() {
        out.push_str(&format!("- aliases: {}\n", entity.aliases.join(", ")));
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

    // Referenced By (omit if empty)
    let refby: Vec<String> = edges
        .incoming
        .iter()
        .filter_map(|id| link_name(id, entities))
        .collect();
    if !refby.is_empty() {
        out.push_str("## Referenced By\n");
        for l in &refby {
            out.push_str(&format!("- {l}\n"));
        }
        out.push('\n');
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

pub fn render_fingerprint(
    entity: &Entity,
    edges: &Edges,
    entities: &BTreeMap<String, Entity>,
    preserved_notes: &str,
) -> [u8; 32] {
    let out_names: Vec<String> = edges
        .outgoing
        .iter()
        .filter_map(|id| entities.get(id).map(|e| e.name.clone()))
        .collect();
    let in_names: Vec<String> = edges
        .incoming
        .iter()
        .filter_map(|id| entities.get(id).map(|e| e.name.clone()))
        .collect();
    combine(&[
        entity.name.as_bytes(),
        entity.created.as_bytes(),
        entity.kind.label().as_bytes(),
        entity.source_path.as_bytes(),
        entity.body.as_bytes(),
        entity.aliases.join(",").as_bytes(),
        entity.symbols.join(",").as_bytes(),
        entity.imports.join(",").as_bytes(),
        out_names.join(",").as_bytes(),
        in_names.join(",").as_bytes(),
        preserved_notes.trim().as_bytes(),
        entity.summary.as_deref().unwrap_or("").as_bytes(),
    ])
}

/// Write via a temp file + rename so an interrupted run can't leave a half-written page.
pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
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
        assert_eq!(read_preserved_notes(&path), "my real notes");
    }

    #[test]
    fn mask_preserves_byte_offsets_through_multibyte_code() {
        // Offsets from the mask index back into the original, so a mask that
        // changed length would panic or slice mid-char.
        let page = "## Body\n`«кодування»`\n\n```\nтіло → приклад\n```\n\n## Notes\nx\n";
        assert_eq!(mask_code(page).len(), page.len());
        assert_eq!(mask_fenced_code(page).len(), page.len());
        let sections = parse_sections(page);
        assert_eq!(sections["Notes"], "x");
        assert!(sections["Body"].contains("«кодування»"));
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
            created: "2026-01-01".into(),
            body: body.into(),
            source_path: format!("{id}.txt"),
            kind: SourceKind::Text,
            content_hash: [0u8; 32],
            summary: None,
            symbols: vec![],
            imports: vec![],
        }
    }
    fn map(v: Vec<Entity>) -> BTreeMap<String, Entity> {
        v.into_iter().map(|e| (e.id.clone(), e)).collect()
    }

    #[test]
    fn related_and_referenced_by_rendered() {
        let ents = map(vec![
            ent("alpha", "Alpha", "mentions Beta"),
            ent("beta", "Beta", "nothing"),
        ]);
        let g = build_graph(&ents);
        let a = render_page(&ents["alpha"], &g.edges["alpha"], &ents, "");
        let b = render_page(&ents["beta"], &g.edges["beta"], &ents, "");
        assert!(a.contains("## Related") && a.contains("[[beta|Beta]]"));
        assert!(b.contains("## Referenced By") && b.contains("[[alpha|Alpha]]"));
        assert!(a.contains("generated"));
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
}
