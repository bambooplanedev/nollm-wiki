//! A page's outline: one line per code symbol with its line range, or a markdown page's headings.
//!
//! The outline is a module map for an agent with a small context: it reads
//! the symbol lines here, then only the `start-end` lines it needs with its
//! own file tool. A page is parsed once into an `Outline`; `render` turns it
//! into text with some categories cut and a cap on item lines, which is how a
//! token budget shrinks it.

use crate::model::{CodeItem, SourceKind};
use crate::query::Wiki;
use crate::rewrite::mask_fenced_code;

/// The last line of a code page whose items are unknown: a wiki compiled
/// before `items.json` existed.
const NO_ITEMS: &str = "no items — recompile with a current wiki for symbol lines";

/// What a page's outline can show, before any cut.
struct Outline<'a> {
    /// `# {title} ({id})`, `{path}  {kind}`, then the summary when there is one.
    header: Vec<String>,
    /// The page's `## Imports` entries, whitespace collapsed.
    imports: Vec<String>,
    body: Body<'a>,
}

enum Body<'a> {
    /// A text page: nothing below the header.
    Empty,
    /// A code page with no items.
    NoItems,
    /// A code page's items, in `items.json` order.
    Items(&'a [CodeItem]),
    /// A markdown page's ATX headings as `(level, text)`, in body order.
    Headings(Vec<(usize, String)>),
}

/// What `render` keeps of an `Outline`.
struct Keep {
    /// Test items.
    tests: bool,
    /// Item docs.
    docs: bool,
    /// The `imports:` block.
    imports: bool,
    /// How many item (or heading) lines, counted after the tests cut. Owner
    /// lines do not count.
    lines: usize,
}

impl Keep {
    const ALL: Keep = Keep {
        tests: true,
        docs: true,
        imports: true,
        lines: usize::MAX,
    };
}

impl Wiki {
    /// The outline of page `id`, or `None` when there is no such page.
    /// `_max_tokens` is not applied yet: the outline is always whole.
    pub fn outline(&self, id: &str, _max_tokens: Option<usize>) -> Option<String> {
        Some(render(&parse(self, id)?, &Keep::ALL))
    }
}

/// Reads page `id`'s entry, items and rendered markdown into an `Outline`.
fn parse<'a>(w: &'a Wiki, id: &str) -> Option<Outline<'a>> {
    let e = w.entry(id)?;
    let mut header = vec![
        format!("# {} ({id})", e.title),
        format!("{}  {}", e.path, e.kind),
    ];
    header.extend(e.summary.as_deref().map(one_line));
    let page = w.page(id).unwrap_or_default();
    let lines: Vec<&str> = page.lines().collect();
    // A code page's body is its source verbatim, so it can hold `## Imports`
    // or `## Body` lines of its own: the generated sections are the ones
    // before the first `## Body`, and the body ends at the last `## Notes`.
    let body_at = lines.iter().position(|l| *l == "## Body");
    let head = &lines[..body_at.unwrap_or(lines.len())];
    let imports = head
        .iter()
        .skip_while(|l| **l != "## Imports")
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter_map(|l| l.strip_prefix("- "))
        .map(one_line)
        .collect();
    let body = match SourceKind::parse(&e.kind) {
        Some(SourceKind::Code { .. }) => match w.items(id) {
            [] => Body::NoItems,
            items => Body::Items(items),
        },
        Some(SourceKind::Markdown) => {
            let start = body_at.map_or(lines.len(), |b| b + 1);
            let end = lines.iter().rposition(|l| *l == "## Notes");
            let body = lines[start..end.filter(|&n| n >= start).unwrap_or(lines.len())].join("\n");
            Body::Headings(
                mask_fenced_code(&body)
                    .lines()
                    .filter_map(heading)
                    .collect(),
            )
        }
        _ => Body::Empty,
    };
    Some(Outline {
        header,
        imports,
        body,
    })
}

/// An ATX heading line as `(level, text)`: one to six `#`, then a space or
/// the line end.
fn heading(line: &str) -> Option<(usize, String)> {
    let text = line.trim_start_matches('#');
    let level = line.len() - text.len();
    let spaced = text.is_empty() || text.starts_with([' ', '\t']);
    ((1..=6).contains(&level) && spaced).then(|| (level, text.trim().to_string()))
}

/// `s` with every whitespace run, newlines included, collapsed to one space.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The outline text with only what `keep` keeps; ends with a newline.
fn render(o: &Outline, keep: &Keep) -> String {
    let mut out = o.header.clone();
    if keep.imports && !o.imports.is_empty() {
        out.push("imports:".into());
        out.extend(o.imports.iter().map(|i| format!("  {i}")));
    }
    match &o.body {
        Body::Empty => {}
        Body::NoItems => out.push(NO_ITEMS.into()),
        Body::Items(items) => {
            let mut run: Option<&str> = None;
            for it in items
                .iter()
                .filter(|i| keep.tests || !i.test)
                .take(keep.lines)
            {
                let owner = it.owner.as_deref();
                if owner != run {
                    out.extend(owner.map(|o| format!("{o}:")));
                    run = owner;
                }
                // The serde name, so the outline says what `items.json` says.
                let kind = serde_json::to_value(it.kind).unwrap_or_default();
                let mut line = format!(
                    "{}{}-{}  {}{}  {}",
                    if owner.is_some() { "  " } else { "" },
                    it.start_line,
                    it.end_line,
                    kind.as_str().unwrap_or_default(),
                    if it.test { " [test]" } else { "" },
                    one_line(&it.signature),
                );
                if let Some(twin) = &it.twin {
                    line += &format!("  → {twin}");
                }
                if let Some(doc) = it.doc.as_deref().filter(|_| keep.docs) {
                    line += &format!("  — {}", one_line(doc));
                }
                out.push(line);
            }
        }
        Body::Headings(headings) => {
            let top = headings.iter().map(|(l, _)| *l).min().unwrap_or(1);
            if !headings.is_empty() {
                out.push("headings:".into());
            }
            out.extend(
                headings
                    .iter()
                    .take(keep.lines)
                    .map(|(l, t)| format!("{}{t}", " ".repeat(2 + 2 * (l - top)))),
            );
        }
    }
    out.join("\n") + "\n"
}
