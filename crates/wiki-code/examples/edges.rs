//! Prints one tab-separated line per call edge of the tree given as the
//! argument: caller, callee, kind, call line.
use wiki_code::links::resolve::{resolve, EdgeKind};
use wiki_code::links::source::load;

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: edges <directory>");
        std::process::exit(2);
    };
    let project = load(arg.as_ref()).unwrap_or_else(|e| {
        eprintln!("edges: {arg}: {e}");
        std::process::exit(1);
    });
    let r = resolve(project);
    let kind = |k| match k {
        EdgeKind::Certain => "certain",
        EdgeKind::OneOf { outside: false } => "one_of",
        EdgeKind::OneOf { outside: true } => "one_of+outside",
    };
    let mut lines: Vec<String> = (r.edges().iter())
        .map(|e| {
            let ((c, cn), (t, tn)) = (r.place(&e.caller), r.place(&e.callee));
            format!("{c}\t{cn}\t{t}\t{tn}\t{}\t{}", kind(e.kind), e.line)
        })
        .collect();
    lines.sort();
    for l in lines {
        println!("{l}");
    }
}
