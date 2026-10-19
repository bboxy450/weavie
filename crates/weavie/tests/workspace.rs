mod common;
use std::process::Command;

use common::{Fixture, binary, git, run, write};
use weavie::session::Session;

#[test]
fn builds_a_rust_diamond_from_all_selected_sources_with_independent_sessions() {
    let fixture = Fixture::new();
    let edit: Vec<_> = fixture.descriptor().repositories.keys().cloned().collect();
    let first = Session::create(
        fixture.target(),
        &fixture.session_path("one"),
        fixture.descriptor(),
        &fixture.graph(),
        None,
        &edit,
    )
    .unwrap();
    let second = Session::create(
        fixture.target(),
        &fixture.session_path("two"),
        fixture.descriptor(),
        &fixture.graph(),
        None,
        &edit,
    )
    .unwrap();
    assert_eq!(first.cache, second.cache);
    assert!(!first.directory.join("src/unrelated").exists());
    assert_eq!(
        std::fs::read_dir(first.directory.join("src"))
            .unwrap()
            .count(),
        5
    );
    assert!(
        std::fs::symlink_metadata(first.directory.join("src/app"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let arguments = vec!["build".into(), "--offline".into(), "--locked".into()];
    first.cargo(&arguments, true).unwrap();
    assert_eq!(
        run(&mut Command::new(binary(&first.directory))).trim(),
        "16"
    );
    write(
        first.directory.join("src/protocol/core/src/lib.rs"),
        "pub fn value() -> u32 { 20 }\n",
    );
    first.cargo(&arguments, true).unwrap();
    assert_eq!(
        run(&mut Command::new(binary(&first.directory))).trim(),
        "70"
    );
    second.cargo(&arguments, true).unwrap();
    assert_eq!(
        run(&mut Command::new(binary(&second.directory))).trim(),
        "16"
    );
    assert!(
        first.status().unwrap()["protocol"]
            .changes
            .as_ref()
            .unwrap()
            .contains("core/src/lib.rs")
    );
    assert!(
        second.status().unwrap()["protocol"]
            .changes
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert!(git(&fixture.repos["protocol"], &["status", "--porcelain"]).is_empty());
    let source_before =
        std::fs::read(first.directory.join("src/protocol/core/src/lib.rs")).unwrap();
    assert!(
        Session::create(
            fixture.target(),
            &first.directory,
            fixture.descriptor(),
            &fixture.graph(),
            None,
            &edit
        )
        .is_err()
    );
    assert_eq!(
        source_before,
        std::fs::read(first.directory.join("src/protocol/core/src/lib.rs")).unwrap()
    );
}

#[test]
fn refuses_path_escape_before_creating_a_session() {
    let fixture = Fixture::new();
    let edit: Vec<_> = fixture.descriptor().repositories.keys().cloned().collect();
    let mut descriptor = fixture.descriptor();
    descriptor
        .repositories
        .get_mut("protocol")
        .unwrap()
        .crates
        .insert("escape".into(), "../outside".into());
    assert!(
        Session::create(
            fixture.target(),
            &fixture.session_path("invalid"),
            descriptor,
            &fixture.graph(),
            None,
            &edit
        )
        .is_err()
    );
    assert!(!fixture.session_path("invalid").exists());
}
