use crate::model::{Arm, ExitKind, File, Flow, MapCall, Step, Target};
use wiki_code::model::{CallSite, CallVia};

/// A call of `name`, its target not visible.
pub fn call(name: &str) -> Step {
    Step::Call(MapCall {
        text: format!("{name}()"),
        site: CallSite {
            name: name.to_string(),
            via: CallVia::Bare,
            args: Some(0),
            line: 1,
        },
        root: None,
        target: Target::Hidden,
    })
}

pub fn exit(kind: ExitKind) -> Step {
    Step::Exit {
        text: "exit".to_string(),
        line: 1,
        kind,
    }
}

pub fn arm(cond: Option<&str>, body: Vec<Step>, falls: bool) -> Arm {
    Arm {
        cond: cond.map(str::to_string),
        line: 1,
        comment: None,
        pre: Vec::new(),
        body,
        falls,
        after: Vec::new(),
    }
}

pub fn branch(arms: Vec<Arm>, breakable: bool) -> Step {
    Step::Branch {
        head: None,
        line: 1,
        arms,
        exhaustive: false,
        breakable,
        unseen: false,
    }
}

pub fn looping(header: &str, body: Vec<Step>, post: Vec<Step>, test_after: bool) -> Step {
    Step::Loop {
        header: header.to_string(),
        line: 1,
        comment: None,
        pre: Vec::new(),
        body,
        label: None,
        endless: false,
        hidden_exit: false,
        orelse: Vec::new(),
        test_after,
        post,
    }
}

pub fn flow_of(steps: Vec<Step>) -> Flow {
    Flow {
        steps,
        ..Flow::default()
    }
}

pub fn file(path: &str, src: &str) -> File {
    crate::source::project([(path.to_string(), src.to_string())])
        .files
        .remove(path)
        .unwrap()
}

fn flow<'f>(f: &'f File, name: &str) -> &'f crate::model::Flow {
    let i = f.code.items.iter().position(|it| it.name == name).unwrap();
    &f.flows[&i]
}

/// One line per step, two spaces per depth.
pub fn flow_lines(path: &str, src: &str, name: &str) -> Vec<String> {
    fn lines(steps: &[Step], depth: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(depth);
        for s in steps {
            match s {
                Step::Branch {
                    head,
                    line,
                    arms,
                    breakable,
                    ..
                } => {
                    let head = head.as_ref().map_or(String::new(), |h| format!(" {h}"));
                    let breakable = if *breakable { " breakable" } else { "" };
                    out.push(format!("{pad}branch{head} @{line}{breakable}"));
                    for a in arms {
                        let mut pre = Vec::new();
                        lines(&a.pre, depth + 1, &mut pre);
                        out.extend(
                            pre.into_iter()
                                .map(|l| format!("{pad}  pre {}", &l[pad.len() + 2..])),
                        );
                        let cond = a.cond.as_deref().unwrap_or("else");
                        let note = a.comment.as_ref().map_or(String::new(), |c| {
                            format!(" # {}", c.text.replace('\n', "\\n"))
                        });
                        let falls = if a.falls { " falls" } else { "" };
                        out.push(format!("{pad}  arm {cond} @{}{falls}{note}", a.line));
                        lines(&a.body, depth + 2, out);
                    }
                    for a in arms.iter().filter(|a| !a.after.is_empty()) {
                        out.push(format!(
                            "{pad}  after {}",
                            a.cond.as_deref().unwrap_or("else")
                        ));
                        lines(&a.after, depth + 2, out);
                    }
                }
                Step::Finally { line, body } => {
                    out.push(format!("{pad}finally @{line}"));
                    lines(body, depth + 1, out);
                }
                Step::Loop {
                    header,
                    line,
                    pre,
                    body,
                    orelse,
                    test_after,
                    post,
                    ..
                } => {
                    let after = if *test_after { " after" } else { "" };
                    out.push(format!("{pad}loop {header} @{line}{after}"));
                    let mut p = Vec::new();
                    lines(pre, depth + 1, &mut p);
                    out.extend(
                        p.into_iter()
                            .map(|l| format!("{pad}  pre {}", &l[pad.len() + 2..])),
                    );
                    lines(body, depth + 1, out);
                    let mut p = Vec::new();
                    lines(post, depth + 1, &mut p);
                    out.extend(
                        p.into_iter()
                            .map(|l| format!("{pad}  post {}", &l[pad.len() + 2..])),
                    );
                    if !orelse.is_empty() {
                        out.push(format!("{pad}  else"));
                        lines(orelse, depth + 2, out);
                    }
                }
                Step::Call(c) => out.push(format!("{pad}call {} @{}", c.text, c.site.line)),
                Step::Exit { text, line, .. } => out.push(format!("{pad}exit {text} @{line}")),
                Step::Nested { name, line, flow } => {
                    out.push(format!("{pad}nested {name} @{line}"));
                    lines(&flow.steps, depth + 1, out);
                }
            }
        }
    }
    let f = file(path, src);
    let mut out = Vec::new();
    lines(&flow(&f, name).steps, 0, &mut out);
    out
}

/// The function's not-drawn counts by label, sorted by label.
pub fn not_drawn(path: &str, src: &str, name: &str) -> Vec<(String, usize)> {
    let f = file(path, src);
    let mut v: Vec<_> = flow(&f, name)
        .not_drawn
        .iter()
        .map(|(k, n)| (k.label().to_string(), *n))
        .collect();
    v.sort();
    v
}
