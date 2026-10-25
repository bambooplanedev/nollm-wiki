//! A page's outline: one line per code symbol with its line range, or a markdown page's headings.
//!
//! The outline is a module map for an agent with a small context: it reads
//! the symbol lines here, then only the `start-end` lines it needs with its
//! own file tool. A page is parsed once into an `Outline`; `render` turns it
//! into text with some categories cut and a cap on item lines, which is how a
//! token budget shrinks it.

use crate::manifest::{largest_fit, token_estimate};
use crate::model::{one_line, CodeItem, SourceKind};
use crate::query::Wiki;
use crate::rewrite::mask_fenced_code;

/// The last line of a code page whose items are unknown: a wiki compiled
/// before `items.json` existed.
const NO_ITEMS: &str = "no items — recompile with a current wiki for symbol lines";

/// What a page's outline can show, before any cut.
struct Outline<'a> {
    /// `# {title} ({id})`, `{path}  {kind}`, then the summary when there is
    /// one; `page_text` puts its `full page …` line first.
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

    /// The items `render` shows, in order: the budget's counts read them here
    /// too, so they always describe what was shown.
    fn items<'a>(&self, items: &'a [CodeItem]) -> impl Iterator<Item = &'a CodeItem> {
        let tests = self.tests;
        items
            .iter()
            .filter(move |i| tests || !i.test)
            .take(self.lines)
    }
}

impl Wiki {
    /// The outline of page `id`, or `None` when there is no such page. Over
    /// `max_tokens`, test items, docs and imports are cut in that order, then
    /// item lines stop where the budget does; a last `…` line says what was
    /// cut. The header and that line are kept even when they do not fit.
    pub fn outline(&self, id: &str, max_tokens: Option<usize>) -> Option<String> {
        Some(fit(&parse(self, id)?, max_tokens))
    }

    /// What `page` shows: the outline, or with `full` the whole page. A full
    /// page over `max_tokens` becomes its outline when it is a code page with
    /// items, else a prefix of whole lines; either way a first or last line
    /// says so and the text fits `max_tokens`.
    pub fn page_text(&self, id: &str, full: bool, max_tokens: Option<usize>) -> Option<String> {
        if !full {
            return self.outline(id, max_tokens);
        }
        let page = self.page(id)?;
        let n = token_estimate(&page);
        let max = match max_tokens {
            Some(max) if n > max => max,
            _ => return Some(page),
        };
        if let Some(mut o) = parse(self, id).filter(|o| matches!(o.body, Body::Items(_))) {
            // A header line, so the budget and its marker's `max_tokens`
            // count it: passing that number back with `full` shows it all.
            let first = format!("full page ~{n} tokens exceeds max_tokens; outline follows");
            o.header.insert(0, first);
            return Some(fit(&o, Some(max)));
        }
        let marker = |chars: usize| {
            format!(
                "… truncated at ~{} of ~{n} tokens\n",
                chars / 4 // token_estimate of the kept prefix
            )
        };
        // One forward pass over a running char count. The marker's digits make
        // fitting non-monotonic, so the last prefix that fits wins.
        let (mut chars, mut end) = (0, 0);
        let (mut best_chars, mut best_end) = (0, 0);
        for line in page.split_inclusive('\n') {
            chars += line.chars().count();
            end += line.len();
            if (chars + marker(chars).chars().count()) / 4 <= max {
                (best_chars, best_end) = (chars, end);
            }
        }
        Some(page[..best_end].to_string() + &marker(best_chars))
    }
}

/// `o` rendered whole, or cut by `budget` when that is over `max_tokens`.
fn fit(o: &Outline, max_tokens: Option<usize>) -> String {
    let full = render(o, &Keep::ALL);
    match max_tokens {
        Some(max) if token_estimate(&full) > max => budget(o, full, max),
        _ => full,
    }
}

/// `o` cut down to `max` tokens; `full` is its whole outline, which does not fit.
fn budget(o: &Outline, full: String, max: usize) -> String {
    let fits = |text: &str| token_estimate(text) <= max;
    let t = token_estimate(&full);
    let marker = |dropped: &[String], more: Option<String>| {
        let mut parts: Vec<String> = Vec::new();
        if !dropped.is_empty() {
            parts.push(format!("dropped: {}", dropped.join(", ")));
        }
        parts.extend(more);
        format!("… {} — pass max_tokens: {t} for all\n", parts.join("; "))
    };
    let items = match o.body {
        Body::Items(items) => items,
        _ => &[],
    };
    let (mut keep, mut dropped, mut text) = (Keep::ALL, Vec::new(), full);
    for what in ["test items", "docs", "imports"] {
        // How many things the cut removes; a page of only tests keeps them.
        let n = match what {
            "test items" => {
                let n = items.iter().filter(|i| i.test).count();
                if n == items.len() {
                    0
                } else {
                    keep.tests = false;
                    n
                }
            }
            "docs" => {
                keep.docs = false;
                keep.items(items).filter(|i| i.doc.is_some()).count()
            }
            _ => {
                keep.imports = false;
                o.imports.len()
            }
        };
        if n > 0 {
            dropped.push(format!("{n} {what}"));
            text = render(o, &keep) + &marker(&dropped, None);
            if fits(&text) {
                return text;
            }
        }
    }
    // The ceiling: the first lines that fit with a marker naming the rest.
    // `None` is a heading, which has no line of its own to name.
    let starts: Vec<Option<usize>> = match &o.body {
        Body::Items(_) => keep.items(items).map(|i| Some(i.start_line)).collect(),
        Body::Headings(h) => vec![None; h.len()],
        _ => return text,
    };
    if starts.is_empty() {
        return text;
    }
    let ceiling = |n: usize| {
        let r = starts.len() - n;
        let more = match starts[n] {
            Some(line) => format!("{r} more items from line {line}"),
            None => format!("{r} more headings"),
        };
        render(o, &Keep { lines: n, ..keep }) + &marker(&dropped, Some(more))
    };
    // Each line makes the text longer, so the counts that fit are a prefix.
    ceiling(largest_fit(starts.len() - 1, |n| fits(&ceiling(n))))
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
    // A wrapped import (`use a::{` …) continues on lines without `- `.
    let mut imports: Vec<String> = Vec::new();
    for l in head
        .iter()
        .skip_while(|l| **l != "## Imports")
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
    {
        match (l.strip_prefix("- "), imports.last_mut()) {
            (Some(i), _) => imports.push(i.to_string()),
            (None, Some(last)) if !l.trim().is_empty() => *last += &format!(" {l}"),
            _ => {}
        }
    }
    let imports = imports.iter().map(|i| one_line(i)).collect();
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
            for it in keep.items(items) {
                let owner = it.owner.as_deref();
                if owner != run {
                    out.extend(owner.map(|o| format!("{o}:")));
                    run = owner;
                }
                let mut line = format!(
                    "{}{}-{}  {}{}  {}",
                    if owner.is_some() { "  " } else { "" },
                    it.start_line,
                    it.end_line,
                    it.kind.as_str(),
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
            // A ceiling of no headings leaves no label either.
            if !headings.is_empty() && keep.lines > 0 {
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
