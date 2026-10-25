//! Core data types: `SourceKind`, `Entity`, `Graph`, `LintReport`, plus `slugify` for page ids and path normalisation.

use crate::hash::{hash_str, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
/// `code` / `docs` groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KindFilter {
    Exact(SourceKind),
    /// Every `code:<lang>`.
    Code,
    /// `markdown` and `text`.
    Docs,
}

impl KindFilter {
    /// The values `parse` accepts, for user-facing error text.
    pub const EXPECTED: &'static str = "code, docs, text, markdown, or code:<lang>";

    pub fn parse(s: &str) -> Option<KindFilter> {
        match s {
            "code" => Some(KindFilter::Code),
            "docs" => Some(KindFilter::Docs),
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
            KindFilter::Code => matches!(k, SourceKind::Code { .. }),
            KindFilter::Docs => matches!(k, SourceKind::Text | SourceKind::Markdown),
        }
    }
}

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
}

impl CodeItemKind {
    /// The serde name, as `items.json` and the outline print it.
    pub fn as_str(self) -> &'static str {
        match self {
            CodeItemKind::Fn => "fn",
            CodeItemKind::Method => "method",
            CodeItemKind::Type => "type",
            CodeItemKind::Value => "value",
            CodeItemKind::Module => "module",
        }
    }
}

/// What a `CodeItem` is. Fields, enum variants and impl headers are not items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeItemKind {
    /// A free function.
    Fn,
    /// A function with an owner: an `impl` or class method, a trait method.
    Method,
    /// A struct, class, enum, trait, interface, union, type alias, or
    /// exported macro.
    Type,
    /// A constant, static, or module-level variable.
    Value,
    /// A module declaration.
    Module,
}

/// One definition of a code page and where it lives. Field order is the
/// `items.json` key order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeItem {
    pub kind: CodeItemKind,
    /// The name as the source writes it.
    pub name: String,
    /// The enclosing scopes, joined the way the language's signatures join
    /// them (`Wiki`, `<T as Display>`, `Outer.Inner`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The one-line signature as the source reads it, with no owner spliced
    /// in, so it can be searched for in the file. Exception: each C++
    /// declarator of a shared declaration gets its own signature, which may
    /// not appear verbatim (`int a, *b;` gives `b` the signature `int *b`).
    pub signature: String,
    /// 1-based, inclusive, in the original file: from the first attached doc
    /// comment or attribute line to the item's last line.
    pub start_line: usize,
    pub end_line: usize,
    /// The first sentence of the item's own doc comment, capped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// The item's whole doc, whitespace collapsed and capped; `None` when it
    /// equals `doc`. Scored at half weight, never shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_text: Option<String>,
    /// Whether the item is part of the page's export surface (`symbols`).
    pub public: bool,
    /// The `path` of the file holding this item's other half, when one is
    /// known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twin: Option<String>,
    /// Whether the item is test code: in a test file (`is_test_path`), inside
    /// a Rust `#[cfg(test)]` module, or a Rust `#[test]` function. Search
    /// shows such items but does not let them lift their page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub test: bool,
    /// The functions this function calls, as `path:start-end`, sorted: the
    /// call sites in its body matched to project items by name (see
    /// `graph::resolve_calls`). Empty for anything but a function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    /// The call sites as the source names them, until `resolve_calls` turns
    /// them into `calls`.
    #[serde(skip)]
    pub call_sites: Vec<CallSite>,
}

/// A call in a function body, as the source writes its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    /// The called name: `f` of `f()`, `a::f()`, `x.f()`.
    pub name: String,
    pub via: CallVia,
}

/// How a call site reaches its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallVia {
    /// `f()`.
    Bare,
    /// `q::f()`: the qualifier's last segment, generics dropped.
    Path(String),
    /// `self.f()`, `this->f()`.
    SelfRecv,
    /// `x.f()`: the receiver when it is a plain name.
    Member(Option<String>),
}

impl CodeItem {
    /// `path:start-end`, the item's lines in its source file.
    pub fn span(&self, path: &str) -> String {
        format!("{path}:{}-{}", self.start_line, self.end_line)
    }

    /// `owner::name`, or `name` when the item has no owner: how search
    /// prints the item, and a whole-name match for it.
    pub fn qualified_name(&self) -> String {
        match &self.owner {
            Some(o) => format!("{o}::{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Whether `rel_path` is a test file in any language: a path component
/// `tests`, or a file name `test_*`, `*_test.*`, `*_tests.*`.
pub fn is_test_path(rel_path: &str) -> bool {
    let mut parts = rel_path.split('/');
    let name = parts.next_back().unwrap_or("");
    let stem = name.split('.').next().unwrap_or("");
    parts.any(|c| c == "tests")
        || name.starts_with("test_")
        || (name.contains('.') && (stem.ends_with("_test") || stem.ends_with("_tests")))
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

/// The last `/`-separated segment of `path`.
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `title_case` of a file stem or directory name, `_` and `-` read as spaces.
pub(crate) fn title_from_slug(s: &str) -> String {
    title_case(&s.replace(['_', '-'], " "))
}

/// `s` with every whitespace run, newlines included, collapsed to one space.
pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Capitalize each whitespace-separated word and lowercase the rest of it.
/// Shared by every name-deriving path — the text extractor's ALL-CAPS first
/// line, the markdown/code filename fallbacks, and `disambiguate_ids`'
/// directory prefixes — so that a page named from a path and a page named
/// from its own content are cased by one rule rather than three copies of it.
pub(crate) fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &ch.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

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
/// it is never empty (and never silently dropped by slug dedup). Single pass.
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
    } else {
        out
    }
}

/// Repo-relative, forward-slash-normalized path string. Falls back to the raw
/// path (lossy) if `path` is not under `root`.
pub fn normalize_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn code_item_kind_names_match_serde() {
        for k in [
            CodeItemKind::Fn,
            CodeItemKind::Method,
            CodeItemKind::Type,
            CodeItemKind::Value,
            CodeItemKind::Module,
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
    }

    #[test]
    fn slugify_lowercases_and_replaces_separators() {
        assert_eq!(slugify("Gradient Descent"), "gradient_descent");
        assert_eq!(slugify("Top-K Sampling"), "top_k_sampling");
        assert_eq!(slugify("KV Cache"), "kv_cache");
    }

    #[test]
    fn slugify_folds_punctuation_and_unicode() {
        assert_eq!(
            slugify("AI News RSS Aggregator → Telegram"),
            "ai_news_rss_aggregator_telegram"
        );
        assert_eq!(slugify("a  --  b"), "a_b");
        assert_eq!(slugify("  →Foo→  "), "foo");
        assert_eq!(slugify("Rate: 5&6"), "rate_5_6");
        assert_eq!(slugify("café"), "cafe");
        // A `|` in a title MUST fold away — the id is both the `<id>.md`
        // filename and the `[[id|display]]` link target, and a pipe in either
        // would break Task 1's split-on-`|` resolution (Task 1 review finding).
        assert_eq!(slugify("Chapter 1 | Overview"), "chapter_1_overview");
        // 'İ' lowercases to `i` plus a combining dot, which is dropped.
        assert_eq!(slugify("İstanbul"), "istanbul");
    }

    #[test]
    fn slugify_treats_decomposed_short_i_like_precomposed() {
        // `й` written as `и` + combining breve, as macOS tools write file names.
        assert_eq!(slugify("Морськии\u{306} маяк"), "morskyi_maiak");
        assert_eq!(slugify("МОРСЬКИИ\u{306} МАЯК"), "morskyi_maiak");
    }

    #[test]
    fn slugify_transliterates_cyrillic() {
        for (name, id) in [
            ("Морський маяк", "morskyi_maiak"),
            ("Щука й Їжак", "shchuka_i_izhak"),
            ("Ґанок", "ganok"),
            ("Юля", "iulia"),
            ("Єва", "ieva"),
            ("Хата", "khata"),
            ("Цех", "tsekh"),
            ("Жук", "zhuk"),
            ("Чашка", "chashka"),
            ("Объём", "obem"),
            ("Мыло", "mylo"),
            ("Эхо", "ekho"),
            ("MCP-сервер v2", "mcp_server_v2"),
        ] {
            assert_eq!(slugify(name), id, "{name}");
        }
    }

    #[test]
    fn slugify_drops_apostrophes_between_cyrillic_letters_only() {
        for name in ["пам'ять", "пам’ять", "памʼять", "'пам'ять'"] {
            assert_eq!(slugify(name), "pamiat", "{name}");
        }
        assert_eq!(slugify("don't"), "don_t");
        assert_eq!(slugify("Пам'ять агента"), "pamiat_ahenta");
    }

    #[test]
    fn slugify_folds_latin_diacritics() {
        assert_eq!(slugify("Straße"), "strasse");
        assert_eq!(slugify("Łódź"), "lodz");
        assert_eq!(slugify("Ærø"), "aero");
        assert_eq!(slugify("naïve"), "naive");
    }

    #[test]
    fn slugify_without_letters_falls_back_to_hash() {
        assert!(slugify("—").starts_with("page_"));
        assert!(slugify("ьъ").starts_with("page_"));
    }

    #[test]
    fn slugify_all_non_alnum_falls_back_to_hash() {
        let s = slugify("→→→");
        assert!(s.starts_with("page_"), "got {s}");
        assert_eq!(s.len(), "page_".len() + 16);
        assert_eq!(s, slugify("→→→")); // deterministic
        assert_ne!(slugify("→→→"), slugify("★★★")); // distinct names → distinct slugs
    }

    #[test]
    fn normalize_path_is_relative_and_forward_slashed() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("/tmp/proj/src/graph.rs");
        assert_eq!(normalize_path(root, p), "src/graph.rs");
    }

    #[test]
    fn sourcekind_label_and_parse_roundtrip() {
        assert_eq!(SourceKind::Text.label(), "text");
        assert_eq!(
            SourceKind::Code {
                lang: "rust".into()
            }
            .label(),
            "code:rust"
        );
        assert_eq!(SourceKind::parse("markdown"), Some(SourceKind::Markdown));
    }
}
