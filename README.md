# weavie

Build Rust projects from a graph of independent repositories. Nix selects the
sources, Git worktrees isolate edits, and symlinks expose the workspace.

```sh
cargo install weavie --locked

weavie open .#app --name fix --edit app,transport
cd .weavie/sessions/fix
weavie cargo test --offline --locked
weavie build
weavie edit protocol
# Edit src/protocol, then build the application against those changes.
weavie cargo run --offline --locked
weavie build --json
weavie status
```

`cargo` runs inside the target's Nix dev shell with local source patches. Use
`weavie cargo --native build` to use an installed Cargo toolchain. `build` invokes
the composition's Nix package with the same source selections. Registry, Git,
renamed dependencies, and repositories containing Cargo workspaces are supported.

The integration test builds six crates from five repositories, including a
diamond dependency, Git and registry patches, and a sibling path dependency. A
shared library edit changes both Cargo and sandboxed Nix binaries from `16` to
`70`; a second session still produces `16`. An unrelated repository stays absent.

## Connect your Nix composition

The Nix helper constructs the build and exports source provenance from the same
values. Install it in the composition:

```sh
weavie nix-lib > weavie.nix
```

For an application in the composition's own Git repository:

```nix
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.transport.url = "github:your-org/transport";
  inputs.protocol.url = "github:your-org/protocol";
  inputs.transport.inputs.protocol.follows = "protocol";

  outputs = inputs@{ nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      graphs = nixpkgs.lib.genAttrs systems (system:
        (import ./weavie.nix) {
          pkgs = nixpkgs.legacyPackages.${system};
          inherit inputs;
          rootRepository = "app";
          cargoLock = inputs.self + "/Cargo.lock";
          repositories = {
            app = {
              input = "self";
              crates.my-app = ".";
              patchSources = [];
            };
            transport = { crates.my-transport = "."; };
            protocol = {
              crates = {
                my-protocol = "crates/protocol";
                my-types = "crates/types";
              };
              patchSources = [ "crates-io" "https://github.com/your-org/protocol" ];
            };
          };
        });
    in {
      packages = builtins.mapAttrs (_: graph: { app = graph.package; }) graphs;
      devShells = builtins.mapAttrs (_: graph: { app = graph.devShell; }) graphs;
      graphRepo.targets = builtins.mapAttrs (_: graph: { app = graph.metadata; }) graphs;
    };
}
```

Replace the repository URLs, crate names, and paths. The application can instead
be an input: declare `inputs.app`, omit `input = "self"`, and use
`cargoLock = inputs.app + "/Cargo.lock"`.

Commit the helper, flake, and lock. Nix's Git fetcher includes files known to Git:

```sh
git add flake.nix weavie.nix Cargo.lock
nix flake lock
git add flake.lock
```

`repositories` describes ownership and Cargo source mapping, not another
dependency graph. Nix still owns input bindings and build dependencies; Cargo
resolves crate versions, features, and transitive dependencies.

See [the workspace guide](docs/workspaces.md) for lock bootstrapping, Git source
URLs, custom builds, snapshot behavior, and constraints. The original
[Nix research](docs/nix-graph-repo.md) documents the underlying mechanisms.

## Inspect and select

```sh
weavie graph                         # Offline flake.lock inspection
weavie graph --from app --json
weavie why protocol --from app
weavie --file /path/to/flake.lock graph
weavie --session .weavie/sessions/fix graph --json  # Effective Nix graph
```

Only sources exported for the selected target appear under `src/`. Selected
editable repositories receive Git worktrees; the rest link to retained Nix source
snapshots. Read-only inputs stay pinned when another session edits the same repo.
Use `--directory` to choose a session location, or global `--session DIRECTORY`
to operate without changing directories.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
nix build .#weavie --no-link -L
nix develop --command cargo test --locked --test nix_workflow -- --ignored --nocapture
```

The Nix tests require a functioning sandbox and fetch the pinned toolchain and
Nixpkgs. Project crate code is compiled from the selected sources. The package
lives in [`crates/weavie`](crates/weavie). MIT licensed.
