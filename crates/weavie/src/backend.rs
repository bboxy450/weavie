//! Nix process boundary shared by graph discovery, builds, and Cargo commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::Graph;

/// Composition, named target, and explicit platform for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub root: PathBuf,
    pub attribute: String,
    pub system: String,
}

/// Data exported by the same Nix helper that constructs the Rust build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub schema_version: u32,
    pub root_repository: String,
    pub cargo_root: String,
    pub repositories: BTreeMap<String, Repository>,
}

/// Source provenance and Cargo bridge for one repository instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Repository {
    pub input: String,
    pub store_path: PathBuf,
    pub crates: BTreeMap<String, String>,
    pub patch_sources: Vec<String>,
}

pub fn nix() -> Command {
    let mut command = Command::new("nix");
    command.args(["--extra-experimental-features", "nix-command flakes"]);
    command
}

pub fn capture(command: &mut Command) -> Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("starting {command:?}"))?;
    ensure!(
        output.status.success(),
        "{command:?} failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

pub fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("starting {command:?}"))?;
    ensure!(status.success(), "{command:?} failed ({status})");
    Ok(())
}

impl Target {
    pub fn resolve(installable: &str) -> Result<Self> {
        let (root, attribute) = installable
            .split_once('#')
            .unwrap_or((installable, "default"));
        let root = if root.is_empty() { "." } else { root };
        let root = Path::new(root)
            .canonicalize()
            .context("open currently requires a local composition directory")?;
        ensure!(
            root.join("flake.nix").is_file(),
            "no flake.nix in {}",
            root.display()
        );
        ensure!(!attribute.is_empty(), "a target name is required after #");
        let system = capture(nix().args([
            "eval",
            "--impure",
            "--raw",
            "--expr",
            "builtins.currentSystem",
        ]))?;
        Ok(Self {
            root,
            attribute: attribute.into(),
            system: String::from_utf8(system.stdout)?.trim().into(),
        })
    }

    pub fn installable(&self) -> String {
        format!("{}#{}", self.root.display(), self.attribute)
    }

    pub fn descriptor(&self, overrides: &[(String, String)]) -> Result<Descriptor> {
        let installable = format!(
            "{}#graphRepo.targets.{}.{}",
            self.root.display(),
            serde_json::to_string(&self.system)?,
            serde_json::to_string(&self.attribute)?
        );
        let output = capture(apply_overrides(nix().args(["eval", "--json", &installable]), overrides))
            .context("evaluating graphRepo target; export metadata using weavie's Nix helper (weavie nix-lib)")?;
        let descriptor: Descriptor =
            serde_json::from_slice(&output.stdout).context("reading graphRepo metadata")?;
        ensure!(
            descriptor.schema_version == 1,
            "unsupported graphRepo metadata version {}",
            descriptor.schema_version
        );
        ensure!(
            descriptor
                .repositories
                .contains_key(&descriptor.root_repository),
            "root repository is missing from graphRepo metadata"
        );
        Ok(descriptor)
    }

    pub fn metadata(&self, overrides: &[(String, String)]) -> Result<serde_json::Value> {
        let output = capture(apply_overrides(
            nix().args(["flake", "metadata", "--json"]).arg(&self.root),
            overrides,
        ))?;
        serde_json::from_slice(&output.stdout).context("reading Nix flake metadata")
    }

    pub fn graph(&self, overrides: &[(String, String)]) -> Result<Graph> {
        Graph::from_json(&self.metadata(overrides)?.to_string())
    }
}

/// Baseline resolution requires an unchanged lock; overrides are ephemeral.
pub fn apply_overrides<'a>(
    command: &'a mut Command,
    overrides: &[(String, String)],
) -> &'a mut Command {
    command.arg("--no-write-lock-file");
    if overrides.is_empty() {
        command.arg("--no-update-lock-file");
    }
    for (input, reference) in overrides {
        command.args(["--override-input", input, reference]);
    }
    command
}
