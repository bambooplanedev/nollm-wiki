//! The questions asked at a terminal. Plain lines: no colour, no cursor control.

use std::io::{self, BufRead, Write};

/// Shows `items` (name, summary, default) as a numbered list and reads one line of numbers or
/// names, separated by commas or spaces. An empty line or end of input takes the defaults; an
/// unknown entry asks again. With `one`, exactly one item must be chosen (an empty line takes
/// the default only when there is exactly one); at end of input with no single default, none.
pub fn choose(
    input: &mut impl BufRead,
    out: &mut impl Write,
    title: &str,
    items: &[(&str, &str, bool)],
    one: bool,
) -> io::Result<Vec<String>> {
    writeln!(out, "{title}")?;
    for (i, (name, summary, default)) in items.iter().enumerate() {
        let mark = if *default { 'x' } else { ' ' };
        writeln!(out, "  {} [{mark}] {name} - {summary}", i + 1)?;
    }
    let defaults: Vec<usize> = (0..items.len()).filter(|&i| items[i].2).collect();
    let ask = if one {
        "One number or name: "
    } else {
        "Numbers or names, separated by commas or spaces (empty: the marked ones): "
    };
    let names = |picked: &[usize]| picked.iter().map(|&i| items[i].0.to_string()).collect();
    loop {
        write!(out, "{ask}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(if one && defaults.len() != 1 {
                Vec::new()
            } else {
                names(&defaults)
            });
        }
        let entries = line.split(|c: char| c == ',' || c.is_whitespace());
        let parsed: Result<Vec<usize>, &str> = (entries.filter(|e| !e.is_empty()))
            .map(|entry| {
                let number = entry.parse::<usize>().ok();
                let number = number.filter(|n| (1..=items.len()).contains(n));
                (number.map(|n| n - 1))
                    .or_else(|| items.iter().position(|it| it.0 == entry))
                    .ok_or(entry)
            })
            .collect();
        let mut picked = match parsed {
            Ok(picked) if picked.is_empty() => defaults.clone(),
            Ok(picked) => picked,
            Err(entry) => {
                writeln!(out, "unknown: {entry}")?;
                continue;
            }
        };
        picked.sort_unstable();
        picked.dedup();
        if one && picked.len() != 1 {
            writeln!(out, "choose one")?;
            continue;
        }
        return Ok(names(&picked));
    }
}

/// Asks `question [Y/n]` (or `[y/N]`): y/yes or n/no, any case; an empty line or end of input
/// takes the default; anything else asks again.
pub fn confirm(
    input: &mut impl BufRead,
    out: &mut impl Write,
    question: &str,
    default_yes: bool,
) -> io::Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        write!(out, "{question} {hint} ")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(default_yes);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(default_yes),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEMS: &[(&str, &str, bool)] = &[
        ("kalka", "the wiki", true),
        ("plaz", "the charts", true),
        ("extra", "something else", false),
    ];
    const SCOPES: &[(&str, &str, bool)] = &[("repo", "committed", false), ("local", "not", false)];

    fn pick(input: &str, one: bool) -> (Vec<String>, String) {
        let mut out = Vec::new();
        let items = if one { SCOPES } else { ITEMS };
        let got = choose(&mut input.as_bytes(), &mut out, "Modules:", items, one).unwrap();
        (got, String::from_utf8(out).unwrap())
    }

    #[test]
    fn empty_line_takes_the_defaults() {
        let (got, out) = pick("\n", false);
        assert_eq!(got, ["kalka", "plaz"]);
        assert_eq!(
            out,
            "Modules:\n  1 [x] kalka - the wiki\n  2 [x] plaz - the charts\n  \
             3 [ ] extra - something else\n\
             Numbers or names, separated by commas or spaces (empty: the marked ones): "
        );
    }

    #[test]
    fn numbers_and_names_both_select() {
        assert_eq!(pick("3, kalka\n", false).0, ["kalka", "extra"]);
        assert_eq!(pick("  2 extra 2\n", false).0, ["plaz", "extra"]);
        assert_eq!(pick("local\n", true).0, ["local"]);
        assert_eq!(pick("1\n", true).0, ["repo"]);
    }

    #[test]
    fn an_unknown_entry_asks_again() {
        let (got, out) = pick("1,9\nplaz,kalak\nplaz\n", false);
        assert_eq!(got, ["plaz"]);
        assert!(out.contains("unknown: 9\n"), "{out}");
        assert!(out.contains("unknown: kalak\n"), "{out}");
        // One answer: two, or none without a single default, ask again.
        let (got, out) = pick("1 2\n\n2\n", true);
        assert_eq!(got, ["local"]);
        assert_eq!(out.matches("choose one\n").count(), 2, "{out}");
    }

    #[test]
    fn end_of_input_takes_the_defaults() {
        assert_eq!(pick("", false).0, ["kalka", "plaz"]);
        assert_eq!(pick("9\n", false).0, ["kalka", "plaz"]);
        // One answer, no single default: nothing, for the caller to refuse.
        assert_eq!(pick("", true).0, [""; 0]);
    }

    fn ask(input: &str, default_yes: bool) -> (bool, String) {
        let mut out = Vec::new();
        let got = confirm(&mut input.as_bytes(), &mut out, "Go on?", default_yes).unwrap();
        (got, String::from_utf8(out).unwrap())
    }

    #[test]
    fn confirm_reads_yes_no_and_the_default() {
        assert_eq!(ask("y\n", false), (true, "Go on? [y/N] ".to_string()));
        assert!(ask("YES\n", false).0);
        assert!(!ask("n\n", true).0);
        assert!(!ask("No\n", true).0);
        assert_eq!(ask("\n", true), (true, "Go on? [Y/n] ".to_string()));
        assert!(!ask("\n", false).0);
        assert!(!ask("", false).0);
        assert!(ask("", true).0);
        let (got, out) = ask("maybe\ny\n", false);
        assert!(got);
        assert_eq!(out, "Go on? [y/N] Go on? [y/N] ");
    }
}
