//! CLI entry point for `wiki`: compile, search, neighbors, lint, serve, and generate subcommands.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use wiki::generator::generate_corpus;
use wiki::model::SourceKind;
use wiki::query::{PackBudget, Wiki};
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
    },
    Neighbors {
        id: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long)]
        max_tokens: Option<usize>,
        #[arg(long)]
        max_nodes: Option<usize>,
        #[arg(long)]
        full: bool,
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Search {
        query: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Lint {
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Serve {
        #[arg(long, default_value = "out")]
        dir: PathBuf,
    },
    Generate {
        dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        files: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
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
        } => {
            let opts = CompileOptions {
                incremental,
                respect_ignore: !no_ignore,
                emit_json,
                jobs: cli.jobs,
                project,
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
                    "Lint: {} broken links, {} orphans",
                    r.lint.broken_links.len(),
                    r.lint.orphans.len()
                );
            }
        }
        Command::Neighbors {
            id,
            depth,
            max_tokens,
            max_nodes,
            full,
            dir,
        } => {
            let w = Wiki::load(&dir).context("load wiki")?;
            let budget = PackBudget {
                max_nodes,
                max_tokens,
                full_neighbors: full,
            };
            let pack = w.neighbors(&id, depth, &budget).context("unknown id")?;
            println!("{}", pack.text);
        }
        Command::Search {
            query,
            kind,
            limit,
            dir,
        } => {
            let w = Wiki::load(&dir).context("load wiki")?;
            let kind = match kind.as_deref() {
                None => None,
                Some(s) => Some(SourceKind::parse(s).with_context(|| {
                    format!("unknown kind {s:?}; expected {}", SourceKind::EXPECTED)
                })?),
            };
            for hit in w.search(&query, kind, limit) {
                println!(
                    "{}\t{}\t{}",
                    hit.id,
                    hit.title,
                    hit.summary.unwrap_or_default()
                );
                if let Some(s) = &hit.snippet {
                    println!("    {s}");
                }
            }
        }
        Command::Lint { dir } => {
            let pages = wiki::lint::load_compiled_pages(&dir).context("read output dir")?;
            let r = wiki::lint::lint(&pages);
            println!(
                "Linted {} pages: {} broken links, {} orphans",
                r.total_pages,
                r.broken_links.len(),
                r.orphans.len()
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
            // Broken links fail the process so `lint` can gate a build (the
            // self-host script does). Orphans are advice, not a defect: a
            // README nothing links to is normal.
            if !r.broken_links.is_empty() {
                std::process::exit(1);
            }
        }
        Command::Serve { dir } => {
            wiki::serve::run(&dir)?;
        }
        Command::Generate { dir, files, seed } => {
            let paths = generate_corpus(&dir, files, seed).context("generate corpus")?;
            println!("Wrote {} files to {}", paths.len(), dir.display());
        }
    }
    Ok(())
}
