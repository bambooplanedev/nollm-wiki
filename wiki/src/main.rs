//! CLI entry point for `wiki`: compile, search, neighbors, page, lint, serve, hint, setup, and generate subcommands.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use wiki::generator::generate_corpus;
use wiki::model::KindFilter;
use wiki::query::{PackBudget, Wiki};
use wiki::setup::{Agent, Scope, SetupOptions};
use wiki::{compile, CompileOptions};

#[derive(Parser)]
#[command(
    name = "wiki",
    version,
    about = "Deterministic wiki compiler for agent-navigable knowledge bases"
)]
struct Cli {
    #[arg(long, global = true)]
    jobs: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Compile {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        incremental: bool,
        #[arg(long)]
        no_ignore: bool,
        #[arg(long)]
        emit_json: bool,
        #[arg(long)]
        watch: bool,
        /// Project name written to index.json, llms.txt and AGENTS.md
        /// (default: the input directory's basename).
        #[arg(long)]
        project: Option<String>,
        /// Also compile an agent memory folder as flagged memory pages:
        /// `local` for this repo's per-user folder, or a directory.
        #[arg(long, value_name = "local|DIR")]
        memory: Option<String>,
    },
    Neighbors {
        id: String,
        /// Link hops (default 1); with `--symbol`, calls deep (default no limit).
        #[arg(long)]
        depth: Option<usize>,
        #[arg(long)]
        max_tokens: Option<usize>,
        #[arg(long)]
        max_nodes: Option<usize>,
        #[arg(long)]
        full: bool,
        /// Only neighbors of this kind: code, docs, text, markdown, or code:<lang>.
        #[arg(long)]
        kind: Option<String>,
        /// A function on the page: list every function it calls instead.
        #[arg(long)]
        symbol: Option<String>,
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    /// Print a page's outline, or with `--full` the whole page.
    Page {
        id: String,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        max_tokens: Option<usize>,
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Search {
        /// Required unless `--tag` is given.
        query: Option<String>,
        /// Only pages carrying this tag; `a` also matches `a/b`. Alone, lists every such page.
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        /// Maximum hits (default 10 with a query; unbounded for a bare `--tag`).
        #[arg(long)]
        limit: Option<usize>,
        /// Output format: `lines` or `json`.
        #[arg(long, value_parser = ["lines", "json"], default_value = "lines")]
        format: String,
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Lint {
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Serve {
        /// Compile this project, watch it, and recompile on every edit.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Wiki to serve (default: `<source>/.wiki` with `--source`, else `out`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Tools to expose, comma-separated, from search, page, neighbors,
        /// lint (default: search,page,neighbors).
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
    },
    /// Print the code locations for a task text; silent when there is nothing to say.
    Hint {
        /// The task text (not used with `--claude-hook`).
        text: Option<String>,
        /// Wiki to read (default: `.wiki`, or `$CLAUDE_PROJECT_DIR/.wiki` with `--claude-hook`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Read a Claude Code `UserPromptSubmit` hook's JSON from stdin.
        #[arg(long)]
        claude_hook: bool,
    },
    /// Install the MCP server and a skill so Claude Code and pi use the wiki in this project.
    Setup {
        /// Agents to set up (default: the ones found).
        #[arg(long, value_delimiter = ',')]
        agent: Option<Vec<Agent>>,
        /// Where the files go (asked when omitted and stdin is a terminal).
        #[arg(long)]
        scope: Option<Scope>,
        /// Also install the prompt hook.
        #[arg(long)]
        hook: bool,
        /// Undo what setup wrote (ignore lines and `.wiki/` stay).
        #[arg(long)]
        remove: bool,
    },
    Generate {
        dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        files: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
}

fn ask_scope() -> Result<Scope> {
    use std::io::IsTerminal;
    anyhow::ensure!(
        std::io::stdin().is_terminal(),
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile {
            input,
            output,
            incremental,
            no_ignore,
            emit_json,
            watch,
            project,
            memory,
        } => {
            let memory = match memory.as_deref() {
                None => None,
                Some("local") => wiki::memory::local_dir(&input),
                Some(dir) => Some(std::path::absolute(dir).context("resolve --memory")?),
            };
            let opts = CompileOptions {
                incremental,
                respect_ignore: !no_ignore,
                emit_json,
                jobs: cli.jobs,
                project,
                memory,
            };
            if watch {
                wiki::watch::watch(&input, &output, &opts).context("watch failed")?;
            } else {
                let r = compile(&input, &output, &opts).context("compile failed")?;
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
            let w = Wiki::load(&dir).context("load wiki")?;
            let kind = match kind.as_deref() {
                None => None,
                Some(s) => Some(KindFilter::parse(s).with_context(|| {
                    format!("unknown kind {s:?}; expected {}", KindFilter::EXPECTED)
                })?),
            };
            let budget = PackBudget {
                max_nodes,
                max_tokens,
                full_neighbors: full,
                kind,
            };
            let rid = w
                .resolve_id(&id)
                .map_err(|near| anyhow::anyhow!(wiki::query::unknown_page(&id, &near)))
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
        Command::Page {
            id,
            full,
            max_tokens,
            dir,
        } => {
            let w = Wiki::load(&dir).context("load wiki")?;
            let rid = w
                .resolve_id(&id)
                .map_err(|near| anyhow::anyhow!(wiki::query::unknown_page(&id, &near)))
                .context("unknown id")?;
            print!(
                "{}",
                w.page_text(&rid, full, max_tokens).context("unknown id")?
            );
        }
        Command::Search {
            query,
            tag,
            kind,
            limit,
            format,
            dir,
        } => {
            if query.is_none() && tag.is_none() {
                anyhow::bail!("search needs a query or --tag");
            }
            let tag = match tag.as_deref() {
                None => None,
                Some(s) => {
                    Some(wiki::query::tag_filter(s).with_context(|| format!("empty tag {s:?}"))?)
                }
            };
            let w = Wiki::load(&dir).context("load wiki")?;
            let kind = match kind.as_deref() {
                None => None,
                Some(s) => Some(KindFilter::parse(s).with_context(|| {
                    format!("unknown kind {s:?}; expected {}", KindFilter::EXPECTED)
                })?),
            };
            let hits = w.search_tagged(
                query.as_deref().unwrap_or(""),
                kind.as_ref(),
                tag.as_deref(),
                limit,
            );
            if format == "json" {
                println!("{}", wiki::serve::search_json(hits));
            } else {
                print!("{}", wiki::serve::search_lines(&hits));
            }
        }
        Command::Lint { dir } => {
            let pages = wiki::lint::load_compiled_pages(&dir).context("read output dir")?;
            let r = wiki::lint::lint(&pages);
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
        Command::Serve { source, dir, tools } => {
            let dir = dir.unwrap_or_else(|| {
                source
                    .as_ref()
                    .map_or_else(|| "out".into(), |s| s.join(".wiki"))
            });
            wiki::serve::run(&dir, source.as_deref(), cli.jobs, tools.as_deref())?;
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
                match wiki::hint::prompt_from_hook_json(&json) {
                    Some(t) => t,
                    None => return Ok(()),
                }
            } else {
                text.context("hint needs TEXT or --claude-hook")?
            };
            let dir = dir.unwrap_or_else(|| {
                match std::env::var_os("CLAUDE_PROJECT_DIR").filter(|_| claude_hook) {
                    Some(p) => PathBuf::from(p).join(".wiki"),
                    None => ".wiki".into(),
                }
            });
            if let Ok(w) = Wiki::load(&dir) {
                print!("{}", wiki::hint::hint_lines(&w, &text));
            }
        }
        Command::Setup {
            agent,
            scope,
            hook,
            remove,
        } => {
            let scope = match scope {
                Some(s) => s,
                None => ask_scope()?,
            };
            let opts = SetupOptions {
                agents: agent,
                scope,
                hook,
                remove,
            };
            let root = std::env::current_dir()?;
            if !wiki::setup::run(&root, &opts, &mut std::io::stdout())? {
                std::process::exit(1);
            }
        }
        Command::Generate { dir, files, seed } => {
            let paths = generate_corpus(&dir, files, seed).context("generate corpus")?;
            println!("Wrote {} files to {}", paths.len(), dir.display());
        }
    }
    Ok(())
}
