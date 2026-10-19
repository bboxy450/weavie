use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use weavie::{Graph, Node};

#[derive(Parser)]
#[command(
    version,
    about = "Explore Nix input graphs without checking out repositories"
)]
struct Cli {
    /// flake.lock or saved Nix metadata JSON; use - for stdin
    #[arg(long, global = true, default_value = "flake.lock", value_name = "FILE")]
    file: PathBuf,
    /// Emit deterministic machine-readable JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the declared source-input graph (not a target's build closure)
    Graph {
        /// Select the input closure below this root-relative input path
        #[arg(long, default_value = ".", value_name = "INPUT_PATH")]
        from: String,
    },
    /// Explain a shortest declared input path to a dependency
    Why {
        /// Root-relative input path, e.g. app/transport
        input: String,
        /// Start at this input; both paths are relative to the original root
        #[arg(long, default_value = ".", value_name = "INPUT_PATH")]
        from: String,
    },
}

fn source_label(node: &Node) -> String {
    node.locked
        .as_ref()
        .map(|locked| serde_json::to_string(locked).expect("JSON object is serializable"))
        .unwrap_or_else(|| "root source (revision is outside flake.lock)".into())
}

fn run(cli: Cli) -> Result<()> {
    let mut contents = String::new();
    if cli.file.as_os_str() == "-" {
        io::stdin()
            .read_to_string(&mut contents)
            .context("reading graph from stdin")?;
    } else {
        contents = std::fs::read_to_string(&cli.file)
            .with_context(|| format!("reading {}", cli.file.display()))?;
    }
    let graph = Graph::from_json(&contents)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    match cli.command {
        Command::Graph { from } => {
            let selected = graph.select(&from)?;
            if cli.json {
                serde_json::to_writer_pretty(&mut out, &selected)?;
                writeln!(out)?;
            } else {
                writeln!(
                    out,
                    "Input graph: {} ({} nodes)",
                    selected.root,
                    selected.nodes.len()
                )?;
                for (id, node) in &selected.nodes {
                    writeln!(out, "{id}: {}", source_label(node))?;
                    for (name, edge) in &node.inputs {
                        write!(out, "  {name} -> {}", edge.node)?;
                        if let Some(path) = &edge.follows {
                            write!(out, " (follows {:?})", path.join("/"))?;
                        }
                        writeln!(out)?;
                    }
                }
            }
        }
        Command::Why { input, from } => {
            let explanation = graph.explain(&input, &from)?;
            if cli.json {
                serde_json::to_writer_pretty(&mut out, &explanation)?;
                writeln!(out)?;
            } else {
                writeln!(
                    out,
                    "{} -> {} (declared inputs)",
                    explanation.from, explanation.to
                )?;
                if explanation.steps.is_empty() {
                    writeln!(out, "  same source instance")?;
                }
                for step in explanation.steps {
                    write!(out, "  {} --{}--> {}", step.from, step.input, step.to)?;
                    if let Some(path) = step.follows {
                        write!(out, " (follows {:?})", path.join("/"))?;
                    }
                    writeln!(out)?;
                }
            }
        }
    }
    out.flush().context("writing output")
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("weavie: {error:#}");
            ExitCode::FAILURE
        }
    }
}
