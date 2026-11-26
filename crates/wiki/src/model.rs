//! Core data types: `SourceKind`, `Entity`, `Graph`, `LintReport`, plus `slugify` for page ids and path normalisation.

use crate::hash::{hash_str, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use wiki_code::model::{
    is_test_path, Arity, CallSite, CallVia, CodeItem, CodeItemKind, PairRole,
};
pub use wiki_code::support::normalize_path;
pub(crate) use wiki_code::support::{
    basename, clip, one_line, title_case, title_from_slug, ITEM_DOC_TEXT_MAX_CHARS,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    #[default]
    Text,
    Markdown,
    Code {
        lang: String,
    },
}

impl SourceKind {
    pub fn label(&self) -> String {
        match self {
            SourceKind::Text => "text".into(),
            SourceKind::Markdown => "markdown".into(),
            SourceKind::Code { lang } => format!("code:{lang}"),
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "text" => Some(SourceKind::Text),
            "markdown" => Some(SourceKind::Markdown),
            other => other.strip_prefix("code:").map(|lang| SourceKind::Code {
                lang: lang.to_string(),
            }),
        }
    }
}

/// What `search` and the CLI/MCP `kind` key accept: one exact kind, or the
/// `code` / `tests` / `docs` groups. In `search`, `code` and `code:<lang>`
/// leave out test pages (`is_test_path`), which `tests` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KindFilter {
    Exact(SourceKind),
    /// Every `code:<lang>`.
    Code,
    /// `markdown` and `text`.
    Docs,
    /// Code pages whose path `is_test_path` passes.
    Tests,
}

impl KindFilter {
    /// The values `parse` accepts, for user-facing error text.
    pub const EXPECTED: &'static str = "code, tests, docs, text, markdown, or code:<lang>";

    pub fn parse(s: &str) -> Option<KindFilter> {
        match s {
            "code" => Some(KindFilter::Code),
            "docs" => Some(KindFilter::Docs),
            "tests" => Some(KindFilter::Tests),
            other => SourceKind::parse(other).map(KindFilter::Exact),
        }
    }

    /// `parse` for a user argument, with the error text callers show.
    pub fn parse_arg(s: &str) -> Result<KindFilter, String> {
        Self::parse(s).ok_or_else(|| format!("unknown kind {s:?}; expected {}", Self::EXPECTED))
    }

    pub fn matches(&self, k: &SourceKind) -> bool {
        match self {
            KindFilter::Exact(x) => x == k,
            // `Tests` also needs the page's path, see `Entry::is_kind`.
            KindFilter::Code | KindFilter::Tests => matches!(k, SourceKind::Code { .. }),
            KindFilter::Docs => matches!(k, SourceKind::Text | SourceKind::Markdown),
        }
    }

    /// Whether a `search` of this kind leaves out test pages: `code` and
    /// `code:<lang>`.
    pub fn drops_tests(&self) -> bool {
        matches!(
            self,
            KindFilter::Code | KindFilter::Exact(SourceKind::Code { .. })
        )
    }

    /// The kind that holds the code pages a search of this kind leaves out:
    /// `tests` for `code` and `code:<lang>`, `code` for `tests`.
    pub fn other(&self) -> Option<KindFilter> {
        match self {
            KindFilter::Tests => Some(KindFilter::Code),
            k if k.drops_tests() => Some(KindFilter::Tests),
            _ => None,
        }
    }
}

pub use techlog::Origin;

#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub created: String,
    /// `YYYY-MM-DD` of the last change, or empty when the source gives none.
    pub updated: String,
    /// From an agent memory folder. Reaches `index.json` only, never the page.
    pub memory: bool,
    /// Where a memory note came from; `Local` on every other page.
    pub origin: Origin,
    /// Names of memory notes this note retires (front-matter `supersedes`).
    pub supersedes: Vec<String>,
    /// The id of the memory note that supersedes this one; set after ids are final.
    pub superseded_by: Option<String>,
    pub body: String,
    pub source_path: String,
    pub kind: SourceKind,
    pub content_hash: [u8; 32],
    pub summary: Option<String>,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
    /// Names of the page's top-level definitions (`ItemKind::FreeDef` and
    /// `FreeValue`: fn, struct, enum, trait, type, const, static, class,
    /// def, module constant), exactly as the tree-sitter `@name` capture
    /// reads them — original case, no owner, no generics. Sorted,
    /// deduplicated. Empty for non-code pages and for pages whose items are
    /// all restricted-visibility. Methods, fields, `mod` declarations and
    /// impl headers are excluded by kind, never by text (see the 2026-09-02
    /// defined-names design).
    pub defined: Vec<String>,
    /// Names of the page's methods: `ItemKind::Member` items whose node is a
    /// function (`code::is_function`) and whose owner is not a trait impl,
    /// exactly as the `@name` capture reads them — original case, no owner. Sorted,
    /// deduplicated. Empty for non-code pages. Inherent-impl and
    /// trait-declaration methods are in; trait-impl methods
    /// (`Default::default`, `Extractor::extract` on the implementing page)
    /// are out by owner, never by text (see the 2026-09-03 prefix-match and
    /// methods design). Python class methods are included
    /// (`function_definition`). JS/TS/Go pages always carry an empty list:
    /// their queries match methods only as `@item`s, never as kept member
    /// `@def`s, so none reaches this list — a scope limit, not a fact about
    /// the file. Search matches these whole, never word-split.
    pub methods: Vec<String>,
    /// The top-level items and methods of a code page, private ones and
    /// tests included, with the lines each spans in the original source
    /// file, so an agent can read just those lines. Sorted by
    /// `(start_line, end_line, name)`. Empty for non-code pages. Written to
    /// `items.json`, not `index.json`.
    ///
    /// Covered: functions and methods; types (struct, class, enum, trait,
    /// interface, union, type alias, exported macro); Rust `mod`s; and
    /// top-level values — Rust `const`/`static`, Python module assignments,
    /// C++ namespace-scope variables, Go package-level `const`/`var` (one
    /// item per name, each grouped spec with its own lines), and JS/TS
    /// top-level variables that hold no function. Not items: fields,
    /// variants, associated consts and types, impl headers, function-local
    /// definitions.
    ///
    /// `public` applies the gate `## Exports` uses. Where the query itself is
    /// that gate (Rust, JS/TS), an item no export pattern matches reads
    /// `false`.
    ///
    /// Known gaps: a Rust `macro_rules!` without `#[macro_export]` is not an
    /// item, because `rust_placement` rejects it before visibility is known;
    /// nor is a function in a Rust `extern` block. A JS/TS method of an
    /// exported class, or an exported variable, reads `public: false`: the
    /// query has no pattern that exports it. An object-literal or
    /// anonymous-class method is a free `fn`, its owner unknown. The names
    /// of one multi-declarator statement (`int a = 1, b;`) share its range.
    pub items: Vec<CodeItem>,
    /// C/C++ data members left out of `items`
    /// (`wiki_code::CodeFile::members_unindexed`).
    pub members_unindexed: usize,
}

impl wiki_code::CodeUnit for Entity {
    fn path(&self) -> &str {
        &self.source_path
    }
    fn lang(&self) -> Option<&str> {
        match &self.kind {
            SourceKind::Code { lang } => Some(lang),
            _ => None,
        }
    }
    fn items(&self) -> &[CodeItem] {
        &self.items
    }
    fn items_mut(&mut self) -> &mut Vec<CodeItem> {
        &mut self.items
    }
    fn defined(&self) -> &[String] {
        &self.defined
    }
}

#[derive(Default, Clone, Debug)]
pub struct Edges {
    pub outgoing: BTreeSet<String>,
    pub incoming: BTreeSet<String>,
    /// The import edges among `outgoing`: pages this page's imports name.
    pub imports: BTreeSet<String>,
    /// The import edges among `incoming`.
    pub imported_by: BTreeSet<String>,
}

#[derive(Default, Clone, Debug)]
pub struct Graph {
    pub edges: BTreeMap<String, Edges>,
    pub pagerank: BTreeMap<String, f64>,
}

#[derive(Default, Clone, Debug)]
pub struct LintReport {
    pub total_pages: usize,
    pub broken_links: Vec<(String, String)>,
    pub orphans: Vec<String>,
    /// Pages (id, tokens) too big for a default `neighbors` pack, sorted by id.
    pub large: Vec<(String, usize)>,
}

/// Days in `month` (0 for a month outside 1..=12); no `year` counts as a leap year.
pub(crate) fn days_in_month(year: Option<u32>, month: u32) -> u32 {
    let leap = year.is_none_or(|y| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => 0,
    }
}

/// Longest field a search or hint line carries, in characters: a run log's
/// summary ran 7k characters on the bench and `search` printed it whole.
pub(crate) const LINE_CHARS: usize = 160;

/// Ascii spelling of one lowercase char: Ukrainian (KMU 2010, position
/// independent) and Russian Cyrillic, plus Latin-1 / Extended-A letters folded
/// to their base letters. `None` for anything else; `Some("")` for a letter
/// that vanishes (soft and hard signs).
#[allow(clippy::match_same_arms)] // a table: Cyrillic and Latin rows stay apart
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "h",
        'ґ' => "g",
        'д' => "d",
        'е' | 'э' | 'ё' => "e",
        'є' => "ie",
        'ж' => "zh",
        'з' => "z",
        'и' | 'ы' => "y",
        'і' | 'ї' | 'й' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ь' | 'ъ' => "",
        'ю' => "iu",
        'я' => "ia",
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĳ' => "ij",
        'ĵ' => "j",
        'ķ' | 'ĸ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// A filesystem/URL-clean id: lowercase, Cyrillic transliterated and Latin
/// diacritics folded to ascii (see `fold_char`), every run of other characters
/// collapsed to a single `_`, no leading/trailing `_`. An apostrophe between
/// two Cyrillic letters and a combining mark vanish without a separator. A name
/// with no letter or digit left falls back to a deterministic `page_<hash>` so
/// it is never empty (and never silently dropped by slug dedup). A slug longer
/// than `MAX_ID_LEN` is cut and suffixed with a hash of the full slug, so it
/// fits a file name and stays distinct. Single pass.
pub fn slugify(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let cyrillic = |c: Option<&char>| c.is_some_and(|c| ('\u{400}'..='\u{4FF}').contains(c));
    let mut out = String::with_capacity(name.len());
    let mut pending_sep = false;
    for (i, &orig) in chars.iter().enumerate() {
        if matches!(orig, '\'' | '\u{2019}' | '\u{2BC}')
            && i > 0
            && cyrillic(chars.get(i - 1))
            && cyrillic(chars.get(i + 1))
        {
            continue;
        }
        for lc in orig.to_lowercase() {
            if ('\u{300}'..='\u{36F}').contains(&lc) {
                continue; // combining mark, also from the lowercase of 'İ'
            }
            let mut buf = [0; 4];
            let folded = if lc == 'и' && chars.get(i + 1) == Some(&'\u{306}') {
                "i" // `й` written as `и` + combining breve
            } else if lc.is_ascii_alphanumeric() {
                &*lc.encode_utf8(&mut buf)
            } else if let Some(f) = fold_char(lc) {
                f
            } else {
                pending_sep = true; // a run of other chars → at most one '_', added lazily
                continue;
            };
            if folded.is_empty() {
                continue;
            }
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            out.push_str(folded);
            pending_sep = false;
        }
    }
    if out.is_empty() {
        format!("page_{}", &to_hex(&hash_str(name))[..16])
    } else if out.len() > MAX_ID_LEN {
        // Too long for a file name: keep a prefix and add a hash of the whole slug.
        let hash = &to_hex(&hash_str(&out))[..16];
        let mut cut = MAX_ID_LEN - hash.len() - 1;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}_{hash}", out[..cut].trim_end_matches('_'))
    } else {
        out
    }
}

/// Longest id in bytes. The id is a file name (`<id>.md`, written via
/// `<id>.md.tmp`), and common file systems cap a name at 255 bytes.
pub const MAX_ID_LEN: usize = 200;

#[cfg(test)]
mod tests;
