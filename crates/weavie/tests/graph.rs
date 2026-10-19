use serde_json::{Value, json};
use weavie::Graph;

const FIXTURE: &str = include_str!("fixtures/flake.lock");

fn parse(value: Value) -> Graph {
    Graph::from_json(&value.to_string()).unwrap()
}

fn error(value: Value) -> String {
    format!("{:#}", Graph::from_json(&value.to_string()).unwrap_err())
}

#[test]
fn resolves_qualified_input_paths_without_collapsing_revisions() {
    let graph = Graph::from_json(FIXTURE).unwrap();
    assert_eq!(graph.root, "composition");
    assert_eq!(graph.resolve_input("app/transport").unwrap(), "transport");
    assert_eq!(
        graph.resolve_input("gateway/transport").unwrap(),
        "transport_old"
    );
    assert_eq!(
        graph.resolve_input("app/transport/protocol").unwrap(),
        "protocol"
    );
    assert_eq!(graph.nodes.len(), 7);
    assert_eq!(
        graph.nodes["app"].inputs["transport"].follows,
        Some(vec!["transport".into()])
    );
}

#[test]
fn preserves_fetcher_options_non_flakes_and_unknown_node_fields() {
    let mut value: Value = serde_json::from_str(FIXTURE).unwrap();
    value["nodes"]["tool"]["parent"] = json!(["app"]);
    let graph = parse(value);
    let protocol = &graph.nodes["protocol"];
    assert!(!protocol.flake);
    assert!(graph.nodes["app"].flake);
    assert_eq!(protocol.locked.as_ref().unwrap()["dir"], "rust");
    assert_eq!(protocol.locked.as_ref().unwrap()["submodules"], true);
    assert!(protocol.original.is_some());
    assert_eq!(graph.nodes["tool"].extra["parent"], json!(["app"]));
}

#[test]
fn preserves_distinct_instances_even_with_identical_source_attributes() {
    let mut value: Value = serde_json::from_str(FIXTURE).unwrap();
    value["nodes"]["transport_old"]["locked"] = value["nodes"]["transport"]["locked"].clone();
    let graph = parse(value);
    assert_eq!(
        graph.resolve_input("gateway/transport").unwrap(),
        "transport_old"
    );
    assert_eq!(graph.resolve_input("app/transport").unwrap(), "transport");
}

#[test]
fn selects_only_the_requested_source_input_closure() {
    let selected = Graph::from_json(FIXTURE).unwrap().select("app").unwrap();
    assert_eq!(selected.root, "app");
    assert_eq!(selected.lock_root, "composition");
    assert_eq!(
        selected
            .nodes
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["app", "protocol", "transport"]
    );
}

#[test]
fn explains_dependencies_from_a_selected_input() {
    let graph = Graph::from_json(FIXTURE).unwrap();
    let explanation = graph.explain("protocol", "app").unwrap();
    assert_eq!(explanation.from, "app");
    assert_eq!(explanation.to, "protocol");
    assert_eq!(
        explanation
            .steps
            .iter()
            .map(|s| s.input.as_str())
            .collect::<Vec<_>>(),
        ["transport", "protocol"]
    );
    assert_eq!(explanation.steps[1].follows, Some(vec!["protocol".into()]));
    assert_eq!(graph.explain("protocol", ".").unwrap().steps.len(), 1);
    assert!(graph.explain("app", "app").unwrap().steps.is_empty());
    assert!(graph.explain("protocol", "tool").is_err());
}

#[test]
fn resolves_follows_chains_and_multicomponent_root_relative_paths() {
    let graph = parse(json!({
        "version": 7, "root": "r", "nodes": {
            "r": { "inputs": { "app": "a", "shared": ["app", "shared"], "alias": ["shared"] } },
            "a": { "inputs": { "shared": "p", "other": ["alias"] } },
            "p": {}
        }
    }));
    assert_eq!(graph.resolve_input("alias").unwrap(), "p");
    assert_eq!(graph.resolve_input("app/other").unwrap(), "p");
}

#[test]
fn empty_follows_refers_to_root_and_valid_source_cycles_terminate() {
    let graph = parse(json!({
        "version": 7, "root": "a", "nodes": {
            "a": { "inputs": { "b": "b" } },
            "b": { "inputs": { "a": [], "self": "b" } }
        }
    }));
    assert_eq!(graph.resolve_input("b/a/b").unwrap(), "b");
    assert_eq!(graph.select("b").unwrap().nodes.len(), 2);
    let explanation = graph.explain(".", "b").unwrap();
    assert_eq!(explanation.steps[0].follows, Some(vec![]));
}

#[test]
fn rejects_alias_cycles_that_never_resolve_to_a_source() {
    let message = error(json!({
        "version": 7, "root": "r", "nodes": {
            "r": { "inputs": { "a": ["b"], "b": ["a"] } }
        }
    }));
    assert!(message.contains("unresolvable follows cycle"), "{message}");
}

#[test]
fn rejects_missing_direct_and_follows_targets() {
    assert!(
        error(json!({
            "version": 7, "root": "r", "nodes": {"r": {"inputs": {"a": "missing"}}}
        }))
        .contains("missing lock node")
    );
    assert!(
        error(json!({
            "version": 7, "root": "r", "nodes": {"r": {"inputs": {"a": ["missing"]}}}
        }))
        .contains("missing input")
    );
}

#[test]
fn rejects_unknown_versions_missing_roots_and_malformed_data() {
    for version in [6, 8] {
        assert!(
            error(json!({"version": version, "root": "r", "nodes": {"r": {}}}))
                .contains("unsupported flake.lock version")
        );
    }
    assert!(error(json!({"version": 7, "root": "r", "nodes": {}})).contains("missing root"));
    assert!(Graph::from_json("{").is_err());
    assert!(Graph::from_json("{}").is_err());
    assert!(
        error(json!({"version": 7, "root": "r", "nodes": {"r": {"inputs": {"a": 2}}}}))
            .contains("invalid flake lock graph")
    );
}

#[test]
fn accepts_saved_metadata_and_emits_deterministic_graph_json() {
    let lock: Value = serde_json::from_str(FIXTURE).unwrap();
    let raw = Graph::from_json(FIXTURE).unwrap();
    let metadata = parse(json!({"path": "/nix/store/example-source", "locks": lock}));
    assert_eq!(
        serde_json::to_string(&raw).unwrap(),
        serde_json::to_string(&metadata).unwrap()
    );
    let graph_json = serde_json::to_value(&raw).unwrap();
    assert_eq!(graph_json["schema_version"], 1);
    assert!(graph_json["nodes"]["composition"].get("locked").is_none());
}

#[test]
fn rejects_bad_selectors_instead_of_guessing_a_repository() {
    let graph = Graph::from_json(FIXTURE).unwrap();
    for path in ["unknown", "app//transport", "/app", "app/", "transport_old"] {
        assert!(graph.resolve_input(path).is_err(), "{path}");
    }
    assert_eq!(graph.resolve_input("").unwrap(), "composition");
    assert_eq!(graph.resolve_input(".").unwrap(), "composition");
}
