//! Dates written in a query: `QueryDate`, month names and the parser.

use crate::model::days_in_month;

/// A calendar date written in a query; `year` is `None` when not given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueryDate {
    pub(super) year: Option<u32>,
    pub(super) month: u32,
    pub(super) day: u32,
}

/// Month names a query may use (lowercase), each with its month number:
/// Ukrainian and Russian nominative and genitive, English full and short.
const MONTH_NAMES: [(u32, &[&str]); 12] = [
    (
        1,
        &["січень", "січня", "январь", "января", "january", "jan"],
    ),
    (
        2,
        &["лютий", "лютого", "февраль", "февраля", "february", "feb"],
    ),
    (3, &["березень", "березня", "март", "марта", "march", "mar"]),
    (
        4,
        &["квітень", "квітня", "апрель", "апреля", "april", "apr"],
    ),
    (5, &["травень", "травня", "май", "мая", "may"]),
    (6, &["червень", "червня", "июнь", "июня", "june", "jun"]),
    (7, &["липень", "липня", "июль", "июля", "july", "jul"]),
    (
        8,
        &["серпень", "серпня", "август", "августа", "august", "aug"],
    ),
    (
        9,
        &[
            "вересень",
            "вересня",
            "сентябрь",
            "сентября",
            "september",
            "sep",
            "sept",
        ],
    ),
    (
        10,
        &["жовтень", "жовтня", "октябрь", "октября", "october", "oct"],
    ),
    (
        11,
        &[
            "листопад",
            "листопада",
            "ноябрь",
            "ноября",
            "november",
            "nov",
        ],
    ),
    (
        12,
        &["грудень", "грудня", "декабрь", "декабря", "december", "dec"],
    ),
];

impl QueryDate {
    /// A real calendar day; 29 February passes without a year.
    fn new(year: Option<u32>, month: u32, day: u32) -> Option<QueryDate> {
        (1..=days_in_month(year, month))
            .contains(&day)
            .then_some(QueryDate { year, month, day })
    }

    /// Whether `updated` (`YYYY-MM-DD`) is this day.
    pub(super) fn matches(&self, updated: &str) -> bool {
        let mut parts = updated.splitn(3, '-').map(|x| x.parse::<u32>().ok());
        let (Some(Some(year)), Some(Some(month)), Some(Some(day))) =
            (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        month == self.month && day == self.day && self.year.is_none_or(|y| y == year)
    }
}

/// `w` as a number of at most `max_len` ASCII digits.
fn number(w: Option<&String>, max_len: usize) -> Option<u32> {
    w.filter(|w| (1..=max_len).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|w| w.parse().ok())
}

/// `w` as a four-digit year.
fn year(w: Option<&String>) -> Option<u32> {
    w.filter(|w| w.len() == 4).and_then(|w| number(Some(w), 4))
}

/// The date one word writes alone: `YYYY-MM-DD`, `D.MM` or `D.MM.YYYY`.
fn numeric_date(word: &str) -> Option<QueryDate> {
    let parts: Vec<&str> = word.split(['.', '-']).collect();
    if !parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let num = |k: usize| parts.get(k).and_then(|p| p.parse::<u32>().ok());
    match (parts.as_slice(), word.contains('-')) {
        ([y, m, d], true) if [y.len(), m.len(), d.len()] == [4, 2, 2] => {
            QueryDate::new(num(0), num(1)?, num(2)?)
        }
        ([d, m], false) if m.len() == 2 && d.len() == 2 => QueryDate::new(None, num(1)?, num(0)?),
        ([d, m, y], false) if m.len() == 2 && d.len() == 2 && y.len() == 4 => {
            QueryDate::new(num(2), num(1)?, num(0)?)
        }
        _ => None,
    }
}

/// The dates a query writes, deduplicated, in query order: `YYYY-MM-DD`,
/// `D.MM[.YYYY]` (two-digit month), `<day> <month name> [year]` and
/// `<month name> <day>[,] [year]`. A month name or a number alone is none.
pub(crate) fn query_dates(query: &str) -> Vec<QueryDate> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .collect();
    let mut out: Vec<QueryDate> = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let found = if let Some((month, _)) = MONTH_NAMES
            .iter()
            .find(|(_, names)| names.contains(&word.as_str()))
        {
            let before = i.checked_sub(1).and_then(|j| number(words.get(j), 2));
            match (before, number(words.get(i + 1), 2)) {
                (Some(day), _) => QueryDate::new(year(words.get(i + 1)), *month, day),
                (None, Some(day)) => QueryDate::new(year(words.get(i + 2)), *month, day),
                _ => None,
            }
        } else {
            numeric_date(word)
        };
        if let Some(date) = found {
            if !out.contains(&date) {
                out.push(date);
            }
        }
    }
    out
}
