//! Read Nix flake input graphs without fetching or checking out repositories.
//!
//! A lock graph describes source inputs, not the build closure of a Nix output.
//! Node identities and fetcher attributes are preserved; `follows` edges resolve
//! from the lock's root, including empty paths that refer to the root itself.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A normalized, deterministic view of a version-7 Nix flake lock graph.
#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    /// Version of weavie's JSON output, independent of the Nix lock version.
    pub schema_version: u32,
    /// Original lock root, against which the retained `follows` paths are defined.
    pub lock_root: String,
    /// The selected root's lock node ID (which need not be named `root`).
    pub root: String,
    /// Lock nodes keyed by their original IDs, never deduplicated by URL.
    pub nodes: BTreeMap<String, Node>,
}

/// A source instance and its declared input bindings.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// Resolved inputs, keyed by their names in the owning flake.
    pub inputs: BTreeMap<String, Edge>,
    /// Exact locked fetcher attributes, including unfamiliar fetcher options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<Map<String, Value>>,
    /// The input's original, potentially unlocked fetcher attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<Map<String, Value>>,
    /// Whether this source is evaluated as a flake (defaults to true in Nix).
    pub flake: bool,
    /// Additional lock-node attributes, retained for provenance.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

/// An input edge with both its resolved identity and original alias path.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Edge {
    /// Destination lock node ID.
    pub node: String,
    /// Root-relative alias path; an empty array follows the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follows: Option<Vec<String>>,
}

/// One declared dependency edge in an explanation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Step {
    /// Source lock node ID.
    pub from: String,
    /// Name of the input in the source flake.
    pub input: String,
    /// Destination lock node ID.
    pub to: String,
    /// Original alias path, when this edge uses `follows`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follows: Option<Vec<String>>,
}

/// A shortest declared input path between two source instances.
#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    /// Version of weavie's explanation JSON.
    pub schema_version: u32,
    /// Starting lock node ID.
    pub from: String,
    /// Destination lock node ID.
    pub to: String,
    /// Ordered dependency edges; empty when the endpoints are identical.
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Lock {
    version: u32,
    root: String,
    nodes: BTreeMap<String, RawNode>,
}

#[derive(Debug, Deserialize)]
struct RawNode {
    #[serde(default)]
    inputs: BTreeMap<String, Input>,
    locked: Option<Map<String, Value>>,
    original: Option<Map<String, Value>>,
    #[serde(default = "default_flake")]
    flake: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn default_flake() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Input {
    Node(String),
    Follows(Vec<String>),
}

type EdgeKey = (String, String);

impl Lock {
    fn resolve_edge(
        &self,
        node: &str,
        input: &str,
        active: &mut BTreeSet<EdgeKey>,
        cache: &mut BTreeMap<EdgeKey, String>,
    ) -> Result<String> {
        let key = (node.to_owned(), input.to_owned());
        if let Some(resolved) = cache.get(&key) {
            return Ok(resolved.clone());
        }
        ensure!(
            active.insert(key.clone()),
            "unresolvable follows cycle at {node}/{input}"
        );
        let edge = self
            .nodes
            .get(node)
            .and_then(|n| n.inputs.get(input))
            .ok_or_else(|| anyhow!("missing input {input:?} on lock node {node:?}"))?;
        let resolved = match edge {
            Input::Node(id) => {
                ensure!(self.nodes.contains_key(id), "missing lock node {id:?}");
                id.clone()
            }
            Input::Follows(path) => {
                let mut current = self.root.clone();
                for part in path {
                    current = self.resolve_edge(&current, part, active, cache)?;
                }
                current
            }
        };
        active.remove(&key);
        cache.insert(key, resolved.clone());
        Ok(resolved)
    }
}

impl Graph {
    /// Parse a `flake.lock` or the JSON output of `nix flake metadata --json`.
    ///
    /// Rejects unsupported lock versions, dangling edges, and unresolvable
    /// `follows` chains. Cycles among resolved source nodes are valid.
    pub fn from_json(json: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(json).context("invalid JSON")?;
        let lock_value = if value.get("nodes").is_some() {
            value
        } else {
            value.get("locks").cloned().ok_or_else(|| {
                anyhow!("expected a flake.lock object or Nix metadata containing 'locks'")
            })?
        };
        let lock: Lock = serde_json::from_value(lock_value).context("invalid flake lock graph")?;
        ensure!(
            lock.version == 7,
            "unsupported flake.lock version {}; expected version 7",
            lock.version
        );
        ensure!(
            lock.nodes.contains_key(&lock.root),
            "missing root lock node {:?}",
            lock.root
        );

        let mut nodes = BTreeMap::new();
        let mut cache = BTreeMap::new();
        for (id, raw) in &lock.nodes {
            let mut inputs = BTreeMap::new();
            for (name, edge) in &raw.inputs {
                let node = lock
                    .resolve_edge(id, name, &mut BTreeSet::new(), &mut cache)
                    .with_context(|| format!("resolving {id}/{name}"))?;
                let follows = match edge {
                    Input::Node(_) => None,
                    Input::Follows(path) => Some(path.clone()),
                };
                inputs.insert(name.clone(), Edge { node, follows });
            }
            nodes.insert(
                id.clone(),
                Node {
                    inputs,
                    locked: raw.locked.clone(),
                    original: raw.original.clone(),
                    flake: raw.flake,
                    extra: raw.extra.clone(),
                },
            );
        }
        Ok(Self {
            schema_version: 1,
            lock_root: lock.root.clone(),
            root: lock.root,
            nodes,
        })
    }

    /// Resolve slash-separated input names from this graph's root.
    ///
    /// `.` and the empty path designate the root. Names are input bindings,
    /// not repository names or lock node IDs.
    pub fn resolve_input(&self, path: &str) -> Result<String> {
        let mut current = self.root.clone();
        if path.is_empty() || path == "." {
            return Ok(current);
        }
        for name in path.split('/') {
            ensure!(!name.is_empty(), "empty input name in path {path:?}");
            let edge = self
                .nodes
                .get(&current)
                .and_then(|n| n.inputs.get(name))
                .ok_or_else(|| anyhow!("no input {name:?} on node {current:?} (path {path:?})"))?;
            current = edge.node.clone();
        }
        Ok(current)
    }

    /// Select the reachable input closure of a root-relative input path.
    ///
    /// This is a source-input closure, not an output's build or runtime closure.
    /// A visited set makes circular source graphs finite.
    pub fn select(&self, input: &str) -> Result<Self> {
        let root = self.resolve_input(input)?;
        let mut pending = VecDeque::from([root.clone()]);
        let mut nodes = BTreeMap::new();
        while let Some(id) = pending.pop_front() {
            if nodes.contains_key(&id) {
                continue;
            }
            let node = self
                .nodes
                .get(&id)
                .ok_or_else(|| anyhow!("missing lock node {id:?}"))?;
            pending.extend(node.inputs.values().map(|e| e.node.clone()));
            nodes.insert(id, node.clone());
        }
        Ok(Self {
            schema_version: 1,
            lock_root: self.lock_root.clone(),
            root,
            nodes,
        })
    }

    /// Explain why `target` is reachable from `from`, using a shortest path.
    ///
    /// Both selectors are input paths from the graph root. Equal-length paths
    /// are selected in input-name order. No build-dependency claim is made.
    pub fn explain(&self, target: &str, from: &str) -> Result<Explanation> {
        let to = self.resolve_input(target)?;
        let start = self.resolve_input(from)?;
        let mut pending = VecDeque::from([start.clone()]);
        let mut seen = BTreeSet::from([start.clone()]);
        let mut previous = BTreeMap::<String, Step>::new();
        while let Some(id) = pending.pop_front() {
            if id == to {
                let mut steps = Vec::new();
                let mut cursor = to.clone();
                while cursor != start {
                    let step = previous
                        .get(&cursor)
                        .ok_or_else(|| anyhow!("incomplete path to {cursor:?}"))?;
                    cursor = step.from.clone();
                    steps.push(step.clone());
                }
                steps.reverse();
                return Ok(Explanation {
                    schema_version: 1,
                    from: start,
                    to,
                    steps,
                });
            }
            let node = self
                .nodes
                .get(&id)
                .ok_or_else(|| anyhow!("missing lock node {id:?}"))?;
            for (input, edge) in &node.inputs {
                if seen.insert(edge.node.clone()) {
                    previous.insert(
                        edge.node.clone(),
                        Step {
                            from: id.clone(),
                            input: input.clone(),
                            to: edge.node.clone(),
                            follows: edge.follows.clone(),
                        },
                    );
                    pending.push_back(edge.node.clone());
                }
            }
        }
        bail!("no declared input path from {from:?} ({start}) to {target:?} ({to})")
    }
}
