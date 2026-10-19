use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use weavie::backend::Target;
use weavie::session::{Session, valid_alias};
use weavie::{Graph, Node};

#[derive(Parser)]
#[command(
    version,
    about = "Build Rust graph-repo workspaces from Nix inputs and Git worktrees"
)]
struct Cli {
    /// flake.lock or saved Nix metadata JSON; use - for stdin
    #[arg(long, global = true, value_name = "FILE", conflicts_with = "session")]
    file: Option<PathBuf>,
    /// Emit deterministic machine-readable JSON
    #[arg(long, global = true)]
    json: bool,
    /// Opened session directory (otherwise discovered from the current directory)
    #[arg(long, global = true, value_name = "DIRECTORY")]
    session: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the sources selected by a Nix graphRepo target
    Open {
        #[arg(default_value = ".#default")]
        target: String,
        #[arg(long, default_value = "default")]
        name: String,
        /// Repository aliases to edit; the root repository is the default
        #[arg(long, value_delimiter = ',')]
        edit: Vec<String>,
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    /// Promote a selected read-only repository to an isolated Git worktree
    Edit { repository: String },
    /// Show source modes, revisions, paths, and working-tree changes
    Status,
    /// Build the Nix target with this session's exact source overrides
    Build {
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Run Cargo with selected repository sources, inside the target's Nix dev shell
    Cargo {
        /// Use the installed Cargo toolchain instead of nix develop
        #[arg(long)]
        native: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Print the Nix integration helper (redirect to weavie.nix in your composition)
    NixLib,
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
    match &cli.command {
        Command::Open {
            target,
            name,
            edit,
            directory,
        } => {
            valid_alias(name)?;
            let target = Target::resolve(target)?;
            let metadata = target.metadata(&[])?;
            let graph = Graph::from_json(&metadata.to_string())?;
            let descriptor = target.descriptor(&[])?;
            let directory = directory
                .clone()
                .unwrap_or_else(|| target.root.join(".weavie/sessions").join(name));
            let session = Session::create(
                target,
                &directory,
                descriptor,
                &graph,
                metadata
                    .get("locked")
                    .and_then(serde_json::Value::as_object)
                    .cloned(),
                edit,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                println!(
                    "Opened {}\nSources: {}",
                    session.directory.display(),
                    session.directory.join("src").display()
                );
            }
            return Ok(());
        }
        Command::Edit { repository } => {
            let mut session = Session::discover(cli.session.as_deref())?;
            session.edit(repository)?;
            println!(
                "Editing {}",
                session.repositories[repository].source_path.display()
            );
            return Ok(());
        }
        Command::Status => {
            let session = Session::discover(cli.session.as_deref())?;
            let status = session.status()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                if !session.ready {
                    println!("Session creation is incomplete");
                }
                for (alias, repo) in status {
                    println!(
                        "{alias}: {} {}\n  {}",
                        if repo.editable { "edit" } else { "read" },
                        repo.revision.as_deref().unwrap_or("source snapshot"),
                        repo.path.display()
                    );
                    if let Some(changes) = repo.changes {
                        print!("{changes}");
                    }
                }
            }
            return Ok(());
        }
        Command::Build { arguments } => {
            let mut arguments = arguments.clone();
            if cli.json {
                arguments.push("--json".into());
            }
            return Session::discover(cli.session.as_deref())?.build(&arguments);
        }
        Command::Cargo { arguments, native } => {
            let arguments = if arguments.is_empty() {
                vec!["build".into()]
            } else {
                arguments.clone()
            };
            return Session::discover(cli.session.as_deref())?.cargo(&arguments, *native);
        }
        Command::NixLib => {
            io::stdout()
                .lock()
                .write_all(include_bytes!("../nix/lib.nix"))?;
            return Ok(());
        }
        _ => {}
    }
    let mut contents = String::new();
    let session = if cli.session.is_some() || cli.file.is_some() {
        cli.session
    } else {
        Session::current_directory()?
    };
    let graph = if let Some(session) = session {
        Session::load(&session)?.graph()?
    } else {
        let file = cli.file.unwrap_or_else(|| PathBuf::from("flake.lock"));
        if file.as_os_str() == "-" {
            io::stdin()
                .read_to_string(&mut contents)
                .context("reading graph from stdin")?;
        } else {
            contents = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
        }
        Graph::from_json(&contents)?
    };
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
        _ => unreachable!("runtime commands returned before offline inspection"),
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
