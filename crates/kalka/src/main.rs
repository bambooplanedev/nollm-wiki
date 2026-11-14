//! CLI entry point for `kalka`: compile, search, neighbors, page, lint, serve, hint, setup, and generate subcommands.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kalka::generator::generate_corpus;
use kalka::model::KindFilter;
use kalka::query::{PackBudget, Wiki};
use kalka::setup::{Agent, Scope, SetupOptions};
use kalka::{compile, CompileOptions};
use std::path::{Path, PathBuf};
use wiki_code::OUT_DIR;

const DIR_HELP: &str = "Wiki to read (default: `.kulman` if that directory exists, else `out`)";

#[derive(Parser)]
#[command(
    name = "kalka",
    version,
    about = "Deterministic wiki compiler for agent-navigable knowledge bases"
)]
struct Cli {
    /// Worker threads for compiling (default: Rayon's, usually the CPU count).
    #[arg(long, global = true)]
    jobs: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a project into a wiki.
    Compile {
        /// Project directory to read.
        input: PathBuf,
        /// Directory the wiki is written to.
        output: PathBuf,
        /// Rewrite only the pages whose content changed.
        #[arg(long)]
        incremental: bool,
        /// Read every file: no .gitignore/.ignore/hidden-file rules, and process and
        /// vendored folders are not skipped.
        #[arg(long)]
        no_ignore: bool,
        /// Also write graph.json.
        #[arg(long)]
        emit_json: bool,
        /// Project name written to index.json, llms.txt and AGENTS.md
        /// (default: the input path's last component, or `wiki` when it has
        /// none, as for `.`).
        #[arg(long)]
        project: Option<String>,
        /// Also compile an agent memory folder as flagged memory pages
        /// (repeatable), or `none` for no memory at all, `.agents/memory` included.
        #[arg(long, value_name = "none|DIR")]
        memory: Vec<String>,
    },
    /// Print a budgeted context pack around a page, or with `--symbol` what a function calls.
    Neighbors {
        /// Page id, title or source path.
        id: String,
        /// Link hops (default 1); with `--symbol`, calls deep (default no limit).
        #[arg(long)]
        depth: Option<usize>,
        /// Token budget (default: none).
        #[arg(long)]
        max_tokens: Option<usize>,
        /// Maximum pages in the pack (default no limit).
        #[arg(long)]
        max_nodes: Option<usize>,
        /// Whole linked pages (outlines for code) instead of one-line summaries.
        #[arg(long)]
        full: bool,
        /// Only neighbors of this kind: code, docs, text, markdown, or code:<lang>.
        #[arg(long)]
        kind: Option<String>,
        /// A function on the page: list every function it calls instead.
        #[arg(long)]
        symbol: Option<String>,
        #[arg(long, help = DIR_HELP)]
        dir: Option<PathBuf>,
    },
    /// List the functions that call a function on a page.
    Callers {
        /// Page id, title or source path.
        id: String,
        /// A function on the page.
        symbol: String,
        /// Caller levels (default 1).
        #[arg(long)]
        depth: Option<usize>,
        /// Token budget (default: none).
        #[arg(long)]
        max_tokens: Option<usize>,
        #[arg(long, help = DIR_HELP)]
        dir: Option<PathBuf>,
    },
    /// Print a page's outline, or with `--full` the whole page.
    Page {
        /// Page id, title or source path.
        id: String,
        /// The whole page instead of the outline.
        #[arg(long)]
        full: bool,
        /// Token budget (default: none).
        #[arg(long)]
        max_tokens: Option<usize>,
        #[arg(long, help = DIR_HELP)]
        dir: Option<PathBuf>,
    },
    /// Find pages and symbols for a query, or step labels in `--docs` files.
    Search {
        /// Required unless `--tag` or `--docs` is given.
        query: Option<String>,
        /// Markdown files (plan, spec, ledger) to search for step labels
        /// instead of the wiki; repeatable. Paths are relative to the
        /// current directory; no compiled wiki is needed.
        #[arg(long, conflicts_with_all = ["tag", "kind", "dir"])]
        docs: Vec<String>,
        /// Only pages carrying this tag; `a` also matches `a/b`. Alone, lists every such page.
        #[arg(long)]
        tag: Option<String>,
        /// Only pages of this kind: code, docs, text, markdown, or code:<lang>.
        #[arg(long)]
        kind: Option<String>,
        /// Maximum hits (default 10 with a query; unbounded for a bare `--tag`).
        #[arg(long)]
        limit: Option<usize>,
        /// Output format: `lines` or `json`.
        #[arg(long, value_parser = ["lines", "json"], default_value = "lines")]
        format: String,
        #[arg(long, help = DIR_HELP)]
        dir: Option<PathBuf>,
    },
    /// Report broken links, orphan pages and large pages; exit 1 on a broken link.
    Lint {
        #[arg(long, help = DIR_HELP)]
        dir: Option<PathBuf>,
    },
    /// Serve a compiled wiki to MCP clients over stdio.
    Serve {
        /// Compile this project, watch it, and recompile on every edit.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Wiki to serve (default: `<source>/.kulman` with `--source`, else
        /// `.kulman` if that directory exists, else `out`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Tools to expose, comma-separated, from search, page, neighbors,
        /// callers, lint (default: search,page,callers).
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
        /// With `--source`, an agent memory folder to include (repeatable), or
        /// `none` for no memory at all, `.agents/memory` included; without it,
        /// Claude Code's memory folder of the project.
        #[arg(long, value_name = "none|DIR", requires = "source")]
        memory: Vec<String>,
    },
    /// Print the code locations for a task text; silent when there is nothing to say.
    Hint {
        /// The task text (not used with `--claude-hook`).
        text: Option<String>,
        /// Wiki to read (default: `.kulman`, or `$CLAUDE_PROJECT_DIR/.kulman` with `--claude-hook`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Read a Claude Code `UserPromptSubmit` hook's JSON from stdin.
        #[arg(long)]
        claude_hook: bool,
    },
    /// Install the MCP server and what tells each agent to use it (Claude Code, pi, Cursor, Codex) in this project, plus the prompt hook for Claude Code and pi.
    Setup {
        /// Agents to set up (default: the ones found, asked at a terminal when none is; Codex only when named, its config is global).
        #[arg(long, value_delimiter = ',')]
        agent: Option<Vec<Agent>>,
        /// Where the files go (asked when omitted and stdin and stdout are terminals).
        #[arg(long)]
        scope: Option<Scope>,
        /// Skip the prompt hook (installed by default for Claude Code and pi).
        #[arg(long)]
        no_hook: bool,
        /// Undo what setup wrote (ignore lines and `.kulman/` stay).
        #[arg(long)]
        remove: bool,
        /// Set up agent memory (the default).
        #[arg(long, conflicts_with = "no_memory")]
        memory: bool,
        /// No agent memory: no `.agents/memory`, and the server runs with `--memory none`.
        #[arg(long)]
        no_memory: bool,
    },
    /// Write a synthetic corpus (same seed, same files).
    Generate {
        /// Directory to write into.
        dir: PathBuf,
        /// Number of files to write.
        #[arg(long, default_value_t = 20)]
        files: usize,
        /// Seed for the generator.
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
}

/// setup asks only when stdin and stdout are both terminals.
fn at_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn ask_scope() -> Result<Scope> {
    anyhow::ensure!(
        at_terminal(),
        "no terminal to ask: pass --scope repo or --scope local"
    );
    println!(
        "Where should setup put its files?\n  1) Add to the repo (committed, for the whole team)\n  2) Local to this checkout (not committed)"
    );
    print!("Choose 1 or 2: ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    match answer.trim() {
        "1" => Ok(Scope::Repo),
        "2" => Ok(Scope::Local),
        _ => anyhow::bail!("choose 1 or 2"),
    }
}

/// No agent was found: which to set up, asked at a terminal (as [`ask_scope`] decides one);
/// without one, none.
fn ask_agents() -> Result<Vec<Agent>> {
    use clap::ValueEnum;
    if !at_terminal() {
        return Ok(Vec::new());
    }
    println!("No agent found in this project. Which should setup wire?");
    for (i, a) in Agent::value_variants().iter().enumerate() {
        if let Some(v) = a.to_possible_value() {
            println!("  {}) {}", i + 1, v.get_name());
        }
    }
    print!("Numbers or names, comma separated (empty for none): ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    kalka::setup::parse_agents(&answer).map_err(anyhow::Error::msg)
}

/// The wiki a command reads: `--dir`, else `./.kulman` if it exists, else `out`.
fn wiki_dir(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(|| {
        let local = PathBuf::from(OUT_DIR);
        if local.is_dir() {
            local
        } else {
            "out".into()
        }
    })
}

/// Exit with a clap usage error (status 2).
fn usage_error(msg: &str) -> ! {
    use clap::CommandFactory;
    Cli::command()
        .error(clap::error::ErrorKind::ArgumentConflict, msg)
        .exit()
}

/// `--memory none` stands alone.
fn no_memory_mix(memory: &[String]) {
    if memory.len() > 1 && memory.iter().any(|m| m == "none") {
        usage_error("--memory none cannot be combined with a folder");
    }
}

fn main() -> Result<()> {
    // Opt-in: with SENTRY_DSN unset the SDK is disabled and sends nothing.
    let mut opts = sentry::ClientOptions::new();
    opts.release = sentry::release_name!();
    let _sentry = sentry::init(opts);
    let result = run();
    if let Err(e) = &result {
        sentry::integrations::anyhow::capture_anyhow(e);
    }
    result
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile {
            input,
            output,
            incremental,
            no_ignore,
            emit_json,
            project,
            memory: memory_arg,
        } => {
            if memory_arg.iter().any(|a| a == "local") {
                usage_error(
                    "--memory local is no longer supported; pass the folder as --memory DIR",
                );
            }
            no_memory_mix(&memory_arg);
            let repo_memory = memory_arg != ["none"];
            let mut memory = Vec::new();
            for arg in memory_arg.iter().filter(|a| *a != "none") {
                let d = kalka::memory::resolve(std::path::Path::new(arg))
                    .context("resolve --memory")?;
                if !d.is_dir() {
                    eprintln!(
                        "warning: --memory {arg}: no such memory folder; compiling without it"
                    );
                }
                memory.push(d);
            }
            let opts = CompileOptions {
                incremental,
                respect_ignore: !no_ignore,
                emit_json,
                jobs: cli.jobs,
                project,
                memory,
                repo_memory,
            };
            let r = kalka::dirlock::exclusive(&output, || compile(&input, &output, &opts))
                .context("compile failed")?;
            println!(
                "Compiled {} pages ({} written) -> {}",
                r.pages_total,
                r.pages_written,
                output.display()
            );
            println!(
                "Lint: {} broken links, {} orphans, {} large",
                r.lint.broken_links.len(),
                r.lint.orphans.len(),
                r.lint.large.len()
            );
            if !no_ignore {
                let mut compiled = opts.memory.clone();
                compiled.push(input.join(kalka::memory::REPO_DIR));
                compiled.push(output.clone());
                let skipped = kalka::walk::skipped(&input, &compiled);
                if !skipped.is_empty() {
                    let rules: Vec<_> = skipped.iter().map(|(r, n)| format!("{r} {n}")).collect();
                    println!("Skipped: {}", rules.join(", "));
                }
            }
        }
        Command::Neighbors {
            id,
            depth,
            max_tokens,
            max_nodes,
            full,
            kind,
            symbol,
            dir,
        } => {
            let w = Wiki::load(&wiki_dir(dir)).context("load wiki")?;
            let kind = kind
                .as_deref()
                .map(KindFilter::parse_arg)
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let budget = PackBudget {
                max_nodes,
                max_tokens,
                full_neighbors: full,
                kind,
            };
            let rid = w
                .resolve(&id)
                .map_err(anyhow::Error::msg)
                .context("unknown id")?;
            if let Some(symbol) = symbol {
                let text = w
                    .calls(&rid, &symbol, depth, max_tokens)
                    .map_err(anyhow::Error::msg)?;
                print!("{text}");
            } else {
                let pack = w
                    .neighbors(&rid, depth.unwrap_or(1), &budget)
                    .context("unknown id")?;
                println!("{}", pack.text);
            }
        }
        Command::Callers {
            id,
            symbol,
            depth,
            max_tokens,
            dir,
        } => {
            let w = Wiki::load(&wiki_dir(dir)).context("load wiki")?;
            let rid = w
                .resolve(&id)
                .map_err(anyhow::Error::msg)
                .context("unknown id")?;
            let text = w
                .callers(&rid, &symbol, Some(depth.unwrap_or(1)), max_tokens)
                .map_err(anyhow::Error::msg)?;
            print!("{text}");
        }
        Command::Page {
            id,
            full,
            max_tokens,
            dir,
        } => {
            let w = Wiki::load(&wiki_dir(dir)).context("load wiki")?;
            let rid = w
                .resolve(&id)
                .map_err(anyhow::Error::msg)
                .context("unknown id")?;
            print!(
                "{}",
                w.page_text(&rid, full, max_tokens)
                    .ok_or_else(|| anyhow::anyhow!(w.page_error(&rid, &id)))?
            );
        }
        Command::Search {
            query,
            docs,
            tag,
            kind,
            limit,
            format,
            dir,
        } => {
            if !docs.is_empty() {
                let (hits, errors, more, fallback) = kalka::labels::doc_search(
                    Path::new("."),
                    &docs,
                    query.as_deref().unwrap_or(""),
                    limit,
                );
                if format == "json" {
                    println!(
                        "{}",
                        kalka::serve::doc_json(&hits, &errors, more, fallback.as_deref())
                    );
                } else {
                    print!(
                        "{}",
                        kalka::labels::doc_lines(&hits, &errors, more, fallback.as_deref())
                    );
                }
                return Ok(());
            }
            if query.is_none() && tag.is_none() {
                anyhow::bail!("search needs a query, --tag or --docs");
            }
            let tag = tag
                .as_deref()
                .map(kalka::query::tag_arg)
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let w = Wiki::load(&wiki_dir(dir)).context("load wiki")?;
            let kind = kind
                .as_deref()
                .map(KindFilter::parse_arg)
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let q = query.as_deref().unwrap_or("");
            let hits = w.search_tagged(q, kind.as_ref(), tag.as_deref(), limit);

            if format == "json" {
                println!("{}", kalka::serve::search_json(hits));
            } else {
                let missing = w.not_found(q, kind.as_ref(), tag.as_deref());
                print!("{}", kalka::serve::search_lines_missing(&hits, &missing));
            }
        }
        Command::Lint { dir } => {
            let pages =
                kalka::lint::load_compiled_pages(&wiki_dir(dir)).context("read output dir")?;
            let r = kalka::lint::lint(&pages);
            println!(
                "Linted {} pages: {} broken links, {} orphans, {} large",
                r.total_pages,
                r.broken_links.len(),
                r.orphans.len(),
                r.large.len()
            );
            // Counts alone are not actionable: print what to go fix. The MCP
            // `lint` tool already returns both lists.
            for (page, link) in &r.broken_links {
                // Quoted, not wikilink-shaped: this text lands in terminals and
                // logs, and a literal `[[...]]` here is one more link-looking
                // string for a renderer (or our own lint) to trip over.
                println!("  broken: {page} -> \"{link}\"");
            }
            for id in &r.orphans {
                println!("  orphan: {id}");
            }
            for (id, tokens) in &r.large {
                println!("  large: {id} ({tokens} tokens)");
            }
            // Broken links fail the process so `lint` can gate a build (the
            // self-host script does). Orphans are advice, not a defect: a
            // README nothing links to is normal.
            if !r.broken_links.is_empty() {
                std::process::exit(1);
            }
        }
        Command::Serve {
            source,
            dir,
            tools,
            memory,
        } => {
            no_memory_mix(&memory);
            let dir = wiki_dir(dir.or_else(|| source.as_ref().map(|s| s.join(OUT_DIR))));
            kalka::serve::run(&dir, source.as_deref(), cli.jobs, tools.as_deref(), &memory)?;
        }
        Command::Hint {
            text,
            dir,
            claude_hook,
        } => {
            let text = if claude_hook {
                let mut json = String::new();
                // A hook must never break the prompt: unusable input is silence.
                if std::io::Read::read_to_string(&mut std::io::stdin(), &mut json).is_err() {
                    return Ok(());
                }
                let Some(t) = kalka::hint::prompt_from_hook_json(&json) else {
                    return Ok(());
                };
                t
            } else {
                text.context("hint needs TEXT or --claude-hook")?
            };
            let dir = dir.unwrap_or_else(|| {
                match std::env::var_os("CLAUDE_PROJECT_DIR").filter(|_| claude_hook) {
                    Some(p) => PathBuf::from(p).join(OUT_DIR),
                    None => OUT_DIR.into(),
                }
            });
            if let Ok(w) = Wiki::load(&dir) {
                print!("{}", kalka::hint::hint_lines(&w, &text));
            }
        }
        Command::Setup {
            agent,
            scope,
            no_hook,
            remove,
            memory: _,
            no_memory,
        } => {
            let scope = scope.map_or_else(ask_scope, Ok)?;
            let root = std::env::current_dir()?;
            // None found: asked at a terminal, else none; `--remove` then removes nothing
            // agent-specific.
            let agents = match agent {
                Some(a) => a,
                None => match kalka::setup::detect(&root) {
                    a if a.is_empty() && !remove => ask_agents()?,
                    a => a,
                },
            };
            let none = agents.is_empty() && !remove;
            let opts = SetupOptions {
                agents: Some(agents),
                scope,
                hook: !no_hook,
                remove,
                memory: !no_memory,
            };
            if !kalka::setup::run(&root, &opts, &mut std::io::stdout())? {
                std::process::exit(1);
            }
            if none {
                eprintln!(
                    "no agent set up: the wiki is in {OUT_DIR}/; run kalka setup --agent … later"
                );
            }
        }
        Command::Generate { dir, files, seed } => {
            let paths = generate_corpus(&dir, files, seed).context("generate corpus")?;
            println!("Wrote {} files to {}", paths.len(), dir.display());
        }
    }
    Ok(())
}
