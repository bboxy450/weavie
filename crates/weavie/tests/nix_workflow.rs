mod common;
use std::process::Command;

use common::{Fixture, git, run, write};
use weavie::session::Session;

fn cli(session: Option<&std::path::Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_weavie"));
    if let Some(directory) = session {
        command.arg("--session").arg(directory);
    }
    command
}

fn build(directory: &std::path::Path) -> String {
    let output = cli(Some(directory))
        .args(["build", "--json", "--", "--print-build-logs"])
        .output()
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    value[0]["outputs"]["out"].as_str().unwrap().to_owned()
}

#[test]
#[ignore = "requires Nix with sandbox support; run in the Nix CI job"]
fn a_self_input_keeps_tracked_dirty_root_edits_in_an_isolated_worktree() {
    let fixture = Fixture::new();
    fixture.configure_nix_self();
    let original = fixture.repos["app"].join("src/main.rs");
    let dirty = "fn main() { println!(\"{}\", demo_transport::value() + demo_codec::value() + demo_metrics::value() + 100); }\n";
    write(&original, dirty);
    let directory = fixture.session_path("self-session");
    run(cli(None)
        .arg("open")
        .arg(format!("{}#app", fixture.repos["app"].display()))
        .arg("--directory")
        .arg(&directory));
    assert_eq!(
        std::fs::read_to_string(directory.join("src/app/src/main.rs")).unwrap(),
        dirty
    );
    run(cli(Some(&directory)).args(["cargo", "build", "--offline", "--locked"]));
    assert_eq!(
        run(&mut Command::new(directory.join("target/debug/demo-app"))).trim(),
        "116"
    );
    let artifact = build(&directory);
    assert_eq!(
        run(&mut Command::new(format!("{artifact}/bin/demo-app"))).trim(),
        "116"
    );
    assert_eq!(std::fs::read_to_string(original).unwrap(), dirty);
    println!(
        "PROOF: self source plus tracked root edits copied to an isolated worktree; Cargo and Nix both produce 116; original worktree untouched"
    );
}

#[test]
#[ignore = "requires Nix with sandbox support; run in the Nix CI job"]
fn nix_and_cargo_build_the_same_multi_repo_sources_and_live_edit() {
    let fixture = Fixture::new();
    fixture.configure_nix();
    let baseline_lock = std::fs::read(fixture.composition.join("flake.lock")).unwrap();
    let first = fixture.session_path("nix-one");
    let second = fixture.session_path("nix-two");
    run(cli(None)
        .arg("open")
        .arg(format!("{}#app", fixture.composition.display()))
        .arg("--directory")
        .arg(&first)
        .args(["--edit", "app,transport"]));
    let session = Session::load(&first).unwrap();
    assert_eq!(
        session.repositories.values().filter(|b| b.editable).count(),
        2
    );
    assert!(!first.join("src/unrelated").exists());
    assert_eq!(std::fs::read_dir(first.join("src")).unwrap().count(), 5);
    for alias in ["codec", "protocol", "metrics"] {
        assert!(first.join("gcroots").join(alias).exists());
        assert!(!first.join("src").join(alias).join(".git").exists());
    }
    run(cli(Some(&first)).args(["cargo", "build", "--offline", "--locked"]));
    assert_eq!(
        run(&mut Command::new(first.join("target/debug/demo-app"))).trim(),
        "16"
    );
    let baseline = build(&first);
    assert_eq!(
        run(&mut Command::new(format!("{baseline}/bin/demo-app"))).trim(),
        "16"
    );
    run(cli(Some(&first)).args(["edit", "protocol"]));
    write(
        first.join("src/protocol/core/src/lib.rs"),
        "pub fn value() -> u32 { 20 }\n",
    );
    run(cli(Some(&first)).args(["cargo", "test", "--offline", "--locked"]));
    run(cli(Some(&first)).args(["cargo", "build", "--offline", "--locked"]));
    assert_eq!(
        run(&mut Command::new(first.join("target/debug/demo-app"))).trim(),
        "70"
    );
    let changed = build(&first);
    assert_ne!(baseline, changed);
    assert_eq!(
        run(&mut Command::new(format!("{changed}/bin/demo-app"))).trim(),
        "70"
    );
    assert_eq!(
        run(&mut Command::new(format!("{baseline}/bin/demo-app"))).trim(),
        "16"
    );
    run(cli(None)
        .arg("open")
        .arg(format!("{}#app", fixture.composition.display()))
        .arg("--directory")
        .arg(&second));
    assert_eq!(
        Session::load(&second)
            .unwrap()
            .repositories
            .values()
            .filter(|b| b.editable)
            .count(),
        1
    );
    run(cli(Some(&second)).args(["cargo", "build", "--offline", "--locked"]));
    assert_eq!(
        run(&mut Command::new(second.join("target/debug/demo-app"))).trim(),
        "16"
    );
    assert_eq!(
        std::fs::read(fixture.composition.join("flake.lock")).unwrap(),
        baseline_lock
    );
    for repo in fixture.repos.values() {
        assert!(git(repo, &["status", "--porcelain"]).is_empty());
    }
    let graph = run(cli(None).current_dir(&first).args(["graph", "--json"]));
    assert!(serde_json::from_str::<serde_json::Value>(&graph).unwrap()["nodes"].is_object());
    println!(
        "PROOF: 6 crates in 5 source repos, 2 worktrees initially, Cargo and sandboxed Nix outputs 16 -> 70, second session 16; unrelated repo absent; composition lock unchanged"
    );
}
