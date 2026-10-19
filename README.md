# weavie

Weave independent repositories into the workspace a task needs.

The intended workflow uses **Nix for dependency selection**, **Git worktrees for
editable sources**, and **symlinks for the visible workspace**. A graph repo is a
versioned composition of repositories; its directory tree is a generated view.

## Available in 0.1.0

The first crate is an offline Nix input-graph inspector. It reads `flake.lock` or
saved `nix flake metadata --json`, resolves `follows`, selects input closures, and
explains dependency paths. It preserves distinct source instances and handles
circular input graphs.

```sh
cargo install weavie --locked

# Run in a project that already has a flake.lock.
weavie graph
weavie graph --from app --json
weavie why protocol --from app
weavie why gateway/transport
weavie --file /path/to/flake.lock graph
```

`--from` and `why` take input names separated by `/`, relative to the original
lock root. `.` selects that root. They do not guess repository names or interpret
output selectors such as `.#app`.

For a remote flake, let Nix produce metadata first:

```sh
nix flake metadata --json --no-update-lock-file github:owner/composition \
  | weavie --file - graph --json
```

weavie itself does not invoke Nix, Git, or the network. The preceding Nix command
can fetch sources. A lock graph describes **declared source inputs**, not the
build closure of a package. This release does not create worktrees, mount
repositories, infer target provenance, or wrap builds yet.

## The graph-repo workflow

These commands describe the next milestone; they are **not implemented**:

```text
weavie open .#app --name transport-fix --edit app,transport --read protocol
weavie edit protocol
weavie build
weavie exec --repo app -- cargo test
weavie status
```

Only selected editable inputs receive Git worktrees. Selected read-only inputs
can point into the Nix store. Builds and development commands must use the same
composition root and local input overrides.

The [Nix and graph-repo research](docs/nix-graph-repo.md) covers the command
surface, worktrees, source provenance, Cargo integration, existing projects, and
the remaining execution tests. Nix is not installed on the initial development
machine; the proposed Nix workflow is documentation-checked, not execution-proven.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run -p weavie -- --file crates/weavie/tests/fixtures/flake.lock why protocol --from app
```

The fixture is a synthetic parser example with invented repositories and
revisions. It is not a buildable Nix composition. The package lives in
[`crates/weavie`](crates/weavie).

MIT licensed.
