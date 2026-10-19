# weavie

Build Rust graph-repo workspaces from Nix inputs, isolated Git worktrees, and
symlinks. Only the selected target's repositories appear in the workspace.

```sh
cargo install weavie --locked

# In a composition configured with the Nix helper:
weavie open .#app --name fix --edit app,transport
cd .weavie/sessions/fix
weavie cargo test --offline --locked
weavie build
weavie edit protocol
weavie cargo run --offline --locked
weavie build --json
weavie status
```

`cargo` runs in the target's Nix development shell with the selected local source
patches. `cargo --native` uses an installed toolchain. `build` applies those same
source selections to the Nix package. Read-only source snapshots have registered
GC roots; editable sources have session branches sharing bare Git object caches.

The included helper builds with `rustPlatform.buildRustPackage`, assembles source
trees, and supports crates.io, Git, renamed, transitive, and sibling path
dependencies, plus repositories containing multiple workspace crates. Toolchain
and system packages can use Nix caches; project crates compile from source.

```sh
weavie nix-lib > weavie.nix
```

Export the helper's package, dev shell, and `graphRepo.targets` metadata from your
flake. The [setup example](https://github.com/bboxy450/weavie#connect-your-nix-composition)
and [workspace guide](https://github.com/bboxy450/weavie/blob/main/docs/workspaces.md)
show the required repository ownership, crate paths, Cargo patch URLs, and locks.
Both independent application inputs and the composition's own `self` source work.

Live Nix tests build six crates from five repositories. A shared crate edit
changes both Cargo and sandboxed Nix output from `16` to `70`; a second session
stays at `16`. Another test preserves tracked dirty `self` edits and builds them
through both paths. Native builds, lock preservation, and unrelated-repo exclusion
are checked too.

Offline graph inspection remains available without Nix:

```sh
weavie graph --from app --json
weavie why protocol --from app
weavie --file /path/to/flake.lock graph
weavie --session /absolute/session graph --json
```

The CLI currently requires Unix symlinks, a local composition, and Git-backed
editable inputs. Nix's Git snapshots include files known to Git: add new files
before building. Sessions keep absolute paths and should stay in place. There is
no automatic worktree deletion or multi-repository commit/push.

The library also exposes the graph, session, and Nix backend modules. MIT licensed.
