# Rust source workspaces

## Ownership and dependency resolution

`weavie open DIRECTORY#TARGET` evaluates
`graphRepo.targets.<current-system>.<target>` from a local flake. The helper
printed by `weavie nix-lib` exports this metadata and constructs `graph.package`
from the same repositories. `graph.devShell` supplies the Rust toolchain and the
package's development inputs.

Repository aliases are workspace directory names. `input` is a slash-separated
flake input binding (defaults to the alias); `self` selects the composition's
own Git repository. `crates` maps Cargo package names to directories relative to
that input's source root. Several crates can belong to one repository. For a
flake in a Git subdirectory, paths are relative to that flake directory.

The CLI preserves lock-node identity rather than deduplicating by repository URL.
One target can select different revisions under different aliases. Cargo patches
must still be unambiguous: one package name per patched source is supported.
Repositories sharing the same lock node should use one shared alias. The CLI
does not infer package provenance from arbitrary derivation names; compositions
must export the helper's metadata.

## Cargo sources and locks

`patchSources` defaults to `[ "crates-io" ]`. For a Git dependency, include the
same repository URL Cargo uses in its `git = "..."` declaration. This is the
Cargo source URL, without the Nix `git+` prefix. A source can patch both crates.io
and a Git URL. Cargo package renaming in `[dependencies]` works normally.

The helper assembles the selected repository trees into a Nix source derivation,
preserves project Cargo configuration, merges local patches, installs the supplied
lock, and builds through `rustPlatform.buildRustPackage`. Registry dependencies
outside the selected repositories are vendored by Nix and compiled from their
source as usual. Rust toolchains and system libraries may come from Nix caches.

The supplied `cargoLock` must reflect the composed source graph. If a pre-existing
lock still selects registry or Git versions of crates that are now local, open
the session and regenerate it first:

```sh
weavie open .#app --name prepare
cd .weavie/sessions/prepare
weavie cargo --native generate-lockfile
weavie cargo --native test --locked
weavie build
```

Using `cargoLock = inputs.app + "/Cargo.lock"` (or `inputs.self`) carries that
tracked lock edit into the Nix build. A composition-owned lock also works; copy
the generated app lock to that file explicitly. Commit lock changes in the
repository that owns the lock. Normal sessions do not write `flake.lock`.

The CLI checks full Cargo metadata before running the requested command. If a
mapped package resolves outside the selected source directory, it fails with the
package and actual path. Check version constraints, patch URLs, and `Cargo.lock`.
Nix builds also check that selected package names resolve to local sources.

New Rust files must be added to Git before a Nix build includes them. Cargo can
read untracked files directly, while Nix's Git source snapshot only includes files
known to Git. `weavie status` shows those working-tree changes.

## Worktrees and snapshots

```text
<composition>/.weavie/cache/git/<URL-hash>.git    shared bare object cache
<session>/worktrees/<alias>                     editable tree or source link
<session>/src/<alias>                           visible symlink
<session>/gcroots/<alias>                       registered Nix root
<session>/cargo-config.toml                     generated Cargo source patches
<session>/cargo-home                            inherited Cargo settings and child-process patches
<session>/weavie-session.json                   recorded ownership and overrides
<session>/target                                session Cargo artifacts
```

Add `.weavie/` to the composition's `.gitignore`. Only repositories selected for
editing get Git worktrees. Other selected sources are store snapshots retained
by registered GC roots. Nix may fetch the source trees needed for metadata and
evaluation; this does not create Git checkouts for them. Every target repository
appears in `src/`, including read-only build dependencies. Inputs omitted by the
target's repository set stay outside the workspace.

Each worktree gets a unique branch. Sessions share Git objects, with serialized
cache allocation, but have independent changes and Cargo build directories.
`weavie edit ALIAS` promotes a read-only source at its original locked commit.
It never switches another session's branch. Files, commits, and branches are not
removed automatically. Existing session paths are rejected rather than replaced.
If opening fails partway, the recorded state and allocated trees remain available
for inspection; `status` identifies incomplete creation.

The session Cargo home carries the same patches to child Cargo processes such as
`cargo-auditable`. It inherits Cargo settings and links to existing registry/Git
caches and credential files without modifying those settings or copying tokens
into repository files. Session configuration files are private to the user.

For `self`, builds use the session's root worktree. Tracked dirty root edits are
copied with a binary Git diff into the new worktree; the original is preserved.
Untracked root files must be added to Git first. Read-only selections use their
original snapshots even if the underlying remote or another checkout changes.

`--override-input` arguments are constructed as process arguments, and local Git
URLs preserve subdirectory, submodule, and LFS options. No machine paths are
written to the committed lock. Sessions currently require Unix symlinks, Git-backed
editable inputs, and a local flake. Keep session directories at their original
paths: Git administrative links, state, and GC roots contain absolute paths.

## Build configuration

`cargoRoot` selects the Cargo workspace directory inside the root repository
(default `"."`). `pname` and `version` set the Nix package label. `buildArgs`
accepts normal `buildRustPackage` options such as `buildInputs`,
`nativeBuildInputs`, `cargoBuildFlags`, and `cargoTestFlags`. The helper owns
`src`, lock selection, and source patching.

For unpatched third-party Git dependencies, pass their required Nix vendor hashes
through `buildArgs.cargoLock.outputHashes`; the helper preserves those options and
sets the lock file itself.

```nix
buildArgs = {
  buildInputs = [ pkgs.openssl ];
  nativeBuildInputs = [ pkgs.pkg-config ];
  cargoBuildFlags = [ "-p" "my-app" "--features" "tls" ];
};
```

`weavie cargo` uses `nix develop ROOT#TARGET --command cargo` and the session patch
configuration. `weavie build` uses `nix build ROOT#TARGET --no-link`. Both apply
the same source overrides. Additional Nix flags follow `--`:

```sh
weavie build -- --print-out-paths --print-build-logs
weavie cargo --native build --offline --locked
weavie --session /absolute/session status --json
```

Changing the target's repository mappings, input topology, or crate ownership
requires opening a new session. Normal code and tracked lock edits in the selected
repositories are live. There is no automatic multi-repository commit or push.

## Executed proof

`tests/nix_workflow.rs` creates real Git repositories, locks a real composition,
opens sessions through the CLI, and asserts executable output. It covers six
Cargo crates in five repositories, a transitive diamond, a multi-crate Cargo
workspace, renamed, registry, Git and sibling-path dependencies, read-only source
promotion, and independent sessions. Both Nix builds and Cargo builds change
from `16` to `70` after editing the shared crate; the original Nix binary and a
second session remain at `16`. The composition lock remains byte-identical.

This proof was executed with Nix 2.34.9 and Rust 1.98.1 on aarch64 Linux, with
`sandbox = true`. The CLI's own `nix build .#weavie` also runs its source-build
tests inside a Nix build, including the cargo-auditable wrapper. Native tests
were executed on macOS with Rust 1.99.0.

`tests/workspace.rs` performs the corresponding source builds with the native
toolchain. The Nix proof runs separately because it requires a Nix installation,
network access for initial toolchain fetches, and sandbox support.

A second Nix test uses `input = "self"` and opens a dirty application checkout.
Its tracked root edits survive into the isolated worktree; Cargo and Nix both
produce `116`, and the original checkout stays unchanged.
