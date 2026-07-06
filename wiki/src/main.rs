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
    verbose: bool,
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
        } => {
            let opts = CompileOptions {
                incremental,
                respect_ignore: !no_ignore,
                emit_json,
                jobs: cli.jobs,
                project: None,
            };
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
            let kind = kind.as_deref().and_then(SourceKind::parse);
            for hit in w.search(&query, kind, limit) {
                println!(
                    "{}\t{}\t{}",
                    hit.id,
                    hit.title,
                    hit.summary.unwrap_or_default()
                );
            }
        }
        Command::Lint { dir } => {
            // Re-read compiled pages and lint in-memory.
            let mut pages = std::collections::BTreeMap::new();
            for entry in std::fs::read_dir(&dir).context("read output dir")? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if stem == "index" || stem == "AGENTS" {
                            continue;
                        }
                        pages.insert(stem.to_string(), std::fs::read_to_string(&path)?);
                    }
                }
            }
            let r = wiki::lint::lint(&pages);
            println!(
                "Linted {} pages: {} broken links, {} orphans",
                r.total_pages,
                r.broken_links.len(),
                r.orphans.len()
            );
        }
        Command::Generate { dir, files, seed } => {
            let paths = generate_corpus(&dir, files, seed).context("generate corpus")?;
            println!("Wrote {} files to {}", paths.len(), dir.display());
        }
    }
    Ok(())
}
