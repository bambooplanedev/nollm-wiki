//! Prints one tab-separated line per call edge of the tree given as the
//! argument: caller, callee, kind.
use wiki_code::links::resolve::{resolve, EdgeKind};
use wiki_code::links::source::load;

fn main() {
    let loaded = std::env::args().nth(1).map(|a| load(a.as_ref()));
    let Some(Ok(project)) = loaded else {
        eprintln!("usage: edges <directory>");
        std::process::exit(2);
    };
    let r = resolve(project);
    let kind = |k| match k {
        EdgeKind::Certain => "certain",
        EdgeKind::OneOf { outside: false } => "one_of",
        EdgeKind::OneOf { outside: true } => "one_of+outside",
    };
    let mut lines: Vec<String> = (r.edges().iter())
        .map(|e| {
            let ((c, cn), (t, tn)) = (r.place(&e.caller), r.place(&e.callee));
            format!("{c}\t{cn}\t{t}\t{tn}\t{}", kind(e.kind))
        })
        .collect();
    lines.sort();
    println!("{}", lines.join("\n"));
}
