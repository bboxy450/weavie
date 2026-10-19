#![allow(dead_code)]
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;
use url::Url;
use weavie::Graph;
use weavie::backend::{Descriptor, Repository, Target};

pub struct Fixture {
    pub temporary: TempDir,
    pub composition: PathBuf,
    pub repos: BTreeMap<String, PathBuf>,
}

pub fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn git(path: &Path, arguments: &[&str]) -> String {
    run(Command::new("git").arg("-C").arg(path).args(arguments))
}

pub fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

pub fn binary(session: &Path) -> PathBuf {
    let native = session.join("target/debug/demo-app");
    if native.is_file() {
        return native;
    }
    for entry in fs::read_dir(session.join("target")).unwrap() {
        let candidate = entry.unwrap().path().join("debug/demo-app");
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("no built demo-app in {}", session.display());
}

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n{deps}\n"
    )
}

fn initialize(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "--initial-branch=main"]);
    git(path, &["config", "user.name", "weavie test"]);
    git(path, &["config", "user.email", "test@example.invalid"]);
}

fn commit(path: &Path) {
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "fixture"]);
}

impl Fixture {
    pub fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let composition = temporary.path().join("composition");
        initialize(&composition);
        let repos: BTreeMap<_, _> = [
            "app",
            "transport",
            "codec",
            "protocol",
            "metrics",
            "unrelated",
        ]
        .into_iter()
        .map(|alias| (alias.to_owned(), temporary.path().join(alias)))
        .collect();
        for repo in repos.values() {
            initialize(repo);
        }
        write(
            repos["protocol"].join("Cargo.toml"),
            "[workspace]\nmembers = [\"core\", \"util\"]\nresolver = \"2\"\n",
        );
        write(
            repos["protocol"].join("core/Cargo.toml"),
            &manifest("demo-core", ""),
        );
        write(
            repos["protocol"].join("core/src/lib.rs"),
            "pub fn value() -> u32 { 2 }\n",
        );
        write(
            repos["protocol"].join("util/Cargo.toml"),
            &manifest("demo-util", ""),
        );
        write(
            repos["protocol"].join("util/src/lib.rs"),
            "pub fn value() -> u32 { 3 }\n",
        );
        write(
            repos["transport"].join("Cargo.toml"),
            &manifest(
                "demo-transport",
                "base = { package = \"demo-core\", path = \"../protocol/core\" }\ndemo-util = \"=0.1.0\"",
            ),
        );
        write(
            repos["transport"].join("src/lib.rs"),
            "pub fn value() -> u32 { base::value() + demo_util::value() }\n",
        );
        let protocol_url = Url::from_directory_path(&repos["protocol"])
            .unwrap()
            .to_string();
        write(
            repos["codec"].join("Cargo.toml"),
            &manifest(
                "demo-codec",
                &format!("demo-core = {{ git = {protocol_url:?} }}"),
            ),
        );
        write(
            repos["codec"].join("src/lib.rs"),
            "pub fn value() -> u32 { demo_core::value() * 2 }\n",
        );
        write(
            repos["metrics"].join("Cargo.toml"),
            &manifest("demo-metrics", ""),
        );
        write(
            repos["metrics"].join("src/lib.rs"),
            "pub fn value() -> u32 { 7 }\n",
        );
        write(
            repos["app"].join("Cargo.toml"),
            &manifest(
                "demo-app",
                "demo-transport = \"=0.1.0\"\ndemo-codec = \"=0.1.0\"\ndemo-metrics = \"=0.1.0\"",
            ),
        );
        write(
            repos["app"].join("src/main.rs"),
            "fn main() { println!(\"{}\", demo_transport::value() + demo_codec::value() + demo_metrics::value()); }\n",
        );
        write(repos["app"].join(".gitignore"), "/target/\n/.weavie/\n");
        write(
            repos["app"].join("Cargo.lock"),
            r#"version = 4
[[package]]
name = "demo-app"
version = "0.1.0"
dependencies = ["demo-codec", "demo-metrics", "demo-transport"]
[[package]]
name = "demo-codec"
version = "0.1.0"
dependencies = ["demo-core"]
[[package]]
name = "demo-core"
version = "0.1.0"
[[package]]
name = "demo-metrics"
version = "0.1.0"
[[package]]
name = "demo-transport"
version = "0.1.0"
dependencies = ["demo-core", "demo-util"]
[[package]]
name = "demo-util"
version = "0.1.0"
"#,
        );
        for (alias, repo) in &repos {
            let dependencies: &[&str] = match alias.as_str() {
                "app" => &["transport", "codec", "metrics"],
                "transport" | "codec" => &["protocol"],
                _ => &[],
            };
            let mut flake = String::from("{\n");
            for dependency in dependencies {
                flake += &format!(
                    "inputs.{dependency}.url = {:?};\n",
                    format!(
                        "git+{}",
                        Url::from_directory_path(&repos[*dependency]).unwrap()
                    )
                );
            }
            flake += "outputs = inputs: {};\n}\n";
            write(repo.join("flake.nix"), &flake);
            commit(repo);
        }
        Self {
            temporary,
            composition,
            repos,
        }
    }

    pub fn descriptor(&self) -> Descriptor {
        let repositories = ["app", "transport", "codec", "protocol", "metrics"]
            .into_iter()
            .map(|alias| {
                let crates = if alias == "protocol" {
                    BTreeMap::from([
                        ("demo-core".into(), "core".into()),
                        ("demo-util".into(), "util".into()),
                    ])
                } else {
                    BTreeMap::from([(format!("demo-{alias}"), ".".into())])
                };
                let patch_sources = if alias == "app" {
                    vec![]
                } else if alias == "protocol" {
                    vec![
                        "crates-io".into(),
                        Url::from_directory_path(&self.repos["protocol"])
                            .unwrap()
                            .to_string(),
                    ]
                } else {
                    vec!["crates-io".into()]
                };
                (
                    alias.into(),
                    Repository {
                        input: alias.into(),
                        store_path: self.repos[alias].clone(),
                        crates,
                        patch_sources,
                    },
                )
            })
            .collect();
        Descriptor {
            schema_version: 1,
            root_repository: "app".into(),
            cargo_root: ".".into(),
            repositories,
        }
    }

    pub fn graph(&self) -> Graph {
        let mut nodes = serde_json::Map::new();
        nodes.insert("root".into(), json!({"inputs": self.repos.keys().map(|a| (a.clone(), a.clone())).collect::<BTreeMap<_,_>>()}));
        for (alias, path) in &self.repos {
            let dependencies: BTreeMap<String, serde_json::Value> = match alias.as_str() {
                "app" => ["transport", "codec", "metrics"]
                    .into_iter()
                    .map(|a| (a.into(), json!([a])))
                    .collect(),
                "transport" | "codec" => BTreeMap::from([("protocol".into(), json!(["protocol"]))]),
                _ => BTreeMap::new(),
            };
            nodes.insert(alias.clone(), json!({"locked": {"type":"git", "url":Url::from_directory_path(path).unwrap().to_string(), "rev":git(path, &["rev-parse", "HEAD"]).trim()}, "inputs": dependencies}));
        }
        Graph::from_json(&json!({"version":7, "root":"root", "nodes":nodes}).to_string()).unwrap()
    }

    pub fn target(&self) -> Target {
        Target {
            root: self.composition.clone(),
            attribute: "app".into(),
            system: "native-test".into(),
        }
    }

    pub fn session_path(&self, name: &str) -> PathBuf {
        self.temporary.path().join(name)
    }

    pub fn configure_nix(&self) {
        let mut flake = String::from("{\ninputs.nixpkgs.url = ");
        flake += &format!(
            "{:?};\n",
            std::env::var("WEAVIE_NIXPKGS")
                .unwrap_or_else(|_| "github:NixOS/nixpkgs/nixos-unstable".into())
        );
        for (alias, path) in &self.repos {
            flake += &format!(
                "inputs.{alias}.url = {:?};\n",
                format!("git+{}", Url::from_directory_path(path).unwrap())
            );
        }
        flake += r#"
inputs.app.inputs.transport.follows = "transport";
inputs.app.inputs.codec.follows = "codec";
inputs.app.inputs.metrics.follows = "metrics";
inputs.transport.inputs.protocol.follows = "protocol";
inputs.codec.inputs.protocol.follows = "protocol";
outputs = inputs@{ nixpkgs, ... }: let
  system = builtins.currentSystem;
  pkgs = nixpkgs.legacyPackages.${system};
  graph = (import ./weavie.nix) {
    inherit pkgs inputs;
    rootRepository = "app";
    cargoLock = inputs.app + "/Cargo.lock";
    repositories = {
      app = { crates.demo-app = "."; patchSources = []; };
      transport = { crates.demo-transport = "."; };
      codec = { crates.demo-codec = "."; };
      metrics = { crates.demo-metrics = "."; };
      protocol = {
        crates = { demo-core = "core"; demo-util = "util"; };
        patchSources = [ "crates-io" "PROTOCOL_URL" ];
      };
    };
  };
in {
  packages.${system}.app = graph.package;
  devShells.${system}.app = graph.devShell;
  graphRepo.targets.${system}.app = graph.metadata;
};
}
"#;
        // Avoid impure system discovery in the flake itself.
        let system = run(Command::new("nix").args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--impure",
            "--raw",
            "--expr",
            "builtins.currentSystem",
        ]));
        flake = flake.replace("builtins.currentSystem", &format!("{:?}", system.trim()));
        flake = flake.replace(
            "PROTOCOL_URL",
            Url::from_directory_path(&self.repos["protocol"])
                .unwrap()
                .as_str(),
        );
        write(self.composition.join("flake.nix"), &flake);
        write(
            self.composition.join("weavie.nix"),
            include_str!("../../nix/lib.nix"),
        );
        commit(&self.composition);
        run(Command::new("nix")
            .args([
                "--extra-experimental-features",
                "nix-command flakes",
                "flake",
                "lock",
            ])
            .arg(&self.composition));
        commit(&self.composition);
    }

    pub fn configure_nix_self(&self) {
        self.configure_nix();
        let composition = fs::read_to_string(self.composition.join("flake.nix")).unwrap();
        let flake = composition
            .lines()
            .filter(|line| !line.starts_with("inputs.app."))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("cargoLock = inputs.app", "cargoLock = inputs.self")
            .replace(
                "app = { crates.demo-app",
                "app = { input = \"self\"; crates.demo-app",
            );
        let root = &self.repos["app"];
        write(root.join("flake.nix"), &flake);
        write(root.join("weavie.nix"), include_str!("../../nix/lib.nix"));
        commit(root);
        run(Command::new("nix")
            .args([
                "--extra-experimental-features",
                "nix-command flakes",
                "flake",
                "lock",
            ])
            .arg(root));
        commit(root);
    }
}
