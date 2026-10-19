# A graph repo with Nix, Git worktrees, and symlinks

Research date: 2026-10-19. Primary reference: Nix 2.34.9 documentation. This
document separates upstream mechanisms, proposed weavie behavior, and executed
checks. The Rust crate currently implements offline input-graph inspection only.

## Shape

A graph repo is a versioned composition of independent repositories. A task
opens a projection of that composition: selected sources appear as symlinks;
editable ones have isolated Git worktrees. Repository ownership and history stay
independent of the visible directory layout.

Use three authorities:

| Layer | Authority | Meaning |
| --- | --- | --- |
| Source selection | `flake.nix` and `flake.lock` | Input instances, revisions, and bindings |
| Build dependencies | Evaluated derivations for an output and system | Concrete build-time dependencies |
| Workspace | weavie session state | Visible sources, worktrees, aliases, local overrides |

Nix explicitly supports multi-repository flakes. Its lock is a graph, with
separate node identities and root-relative `follows` paths. Empty `follows`
paths refer to the root; circular source graphs are possible. The root's own
revision is absent from its lock. Record the composition commit separately.
[Nix flake format and lock semantics](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-flake#lock-files).

**Design decision:** directory structure is a view, never a second dependency
manifest. Preserve repository identity, source revision/options, lock-node
instance, and mutable worktree identity separately. Sharing Git objects must not
merge two graph instances that have different input bindings.

## What selective loading means

Use four independent states: known, available locally, visible, editable.
Learning a locked input exists needs no Git clone. Browsing can use an immutable
source snapshot. Editing allocates a worktree. Build dependencies can remain
outside the visible workspace.

The product promise should be: **only explicitly selected sources appear, and
only explicitly editable sources get worktrees**. Do not promise that Nix
evaluation downloads no other source: evaluating imported package definitions
can require source material. A package's input closure also includes tooling,
fetchers, and other objects that are not repositories a developer should open.

## Nix surface

Start with subprocess adapters and parse JSON into versioned internal types.
Do not link evaluator internals before profiling shows a need.

| Operation | Command shape | Boundary |
| --- | --- | --- |
| Offline source inventory | Read `flake.lock` | Current crate; no evaluation or fetch |
| Resolve a flake | `nix flake metadata --json --no-update-lock-file ROOT` | May fetch; `locks` contains the lock graph |
| Locate source snapshots | `nix flake archive --json --dry-run --no-update-lock-file ROOT` | Inventory, not evidence paths exist |
| Materialize one source | `nix flake prefetch --json LOCKED_REF` | Fetch selected reference |
| Evaluate a target graph | `nix derivation show --recursive --no-update-lock-file ROOT#TARGET` | Build graph, not just input graph |
| Build or substitute | `nix build --json --no-link ROOT#TARGET` | Realize outputs |
| Run in a dev environment | `nix develop ROOT#SHELL --command PROGRAM ARGS...` | Shell selection must match the target's integration |
| Inspect environment | `nix print-dev-env --json ROOT#SHELL` | Variables and Bash functions |
| Inspect realized closure | `nix path-info --recursive --json STORE_PATH` | Does not realize missing objects |

`metadata` resolves/fetches the root and exposes `locks`. `archive --dry-run`
documents source-path inventory without fetching all those sources. `prefetch`
fetches the supplied source, including non-flakes. Prefer it to full recursive
archiving for deliberate source opening.
[Metadata](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-flake-metadata),
[archive](https://nix.dev/manual/nix/2.32/command-ref/new-cli/nix3-flake-archive),
[prefetch](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-flake-prefetch).

Derivation traversal is output-specific. Nix 2.34's documented derivation schema
is version 4, using `inputs.drvs` and `inputs.srcs`; Nix 2.32 documented version 3
with `inputDrvs` and `inputSrcs`. Store-path representation also varies. Capture
real fixtures for each supported version and reject unfamiliar formats.
[Derivation traversal](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-derivation-show),
[version 4 schema](https://nix.dev/manual/nix/2.34/protocols/json/derivation/),
[version 3 schema](https://nix.dev/manual/nix/2.32/command-ref/new-cli/nix3-derivation-show).

Builds can realize outputs; environment commands expose a build/development
environment. A dev shell alone does not rewrite language-level dependency
selection. `path-info` fails for absent outputs instead of building them.
[Build](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-build),
[develop](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-develop),
[print-dev-env](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-print-dev-env),
[path-info](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-path-info).

**Design decision:** ordinary opening consumes the current composition. Updating
pins is a separate explicit action. Remote graph discovery uses Nix metadata;
local lock inspection stays available without Nix. Treat all lock-file writes
as observable operations.

## Composition and local overrides

Illustrative composition, requiring real repository URLs and matching outputs:

```nix
{
  inputs = {
    app.url = "github:example/app";
    transport.url = "github:example/transport";
    protocol = { url = "github:example/protocol"; flake = false; };
    app.inputs.transport.follows = "transport";
    transport.inputs.protocol.follows = "protocol";
  };

  outputs = { app, ... }: {
    packages.x86_64-linux.app = app.packages.x86_64-linux.default;
    devShells.x86_64-linux.app = app.devShells.x86_64-linux.default;
  };
}
```

Proposed invocation after a session allocates a transport worktree:

```sh
nix build /absolute/composition#app \
  --override-input transport git+file:///absolute/worktrees/fix/transport \
  --json --no-link
```

The override changes an input binding; it reaches consumers configured to follow
that binding. Independently pinned instances need their own qualified override.
Nested paths such as `app/transport` are supported, and `--override-input`
implies `--no-write-lock-file`.
[Input override options](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-derivation-show#flake-related-options).

For live Git edits, a local `git+file:` reference without `rev` or `ref` includes
working-tree contents known to Git. New files must be added to the index; a pinned
revision instead selects committed content. Preserve fetcher options such as
`dir`, submodules, and LFS when translating a source into a local override.
[Git flake references](https://nix.dev/manual/nix/2.32/command-ref/new-cli/nix3-flake#types).

**Design decision:** graph, build, and exec share one absolute composition root
and one effective override set. Local paths never enter the committed lock.
Baseline CI disallows lock updates; development overrides intentionally change
resolution, so those flag policies cannot be blindly identical. Changes to
`flake.nix`, source filters, and manifests invalidate graph/provenance caches.

## Git worktrees and the visible filesystem

Proposed storage layout:

```text
cache/git/<repository-id>.git             shared bare Git object cache
worktrees/<session>/<instance>/           editable checkout
sessions/<session>/src/<alias>            symlink to worktree or store source
sessions/<session>/state.json             root, target, overrides, owned paths
```

Allocate a worktree at the locked commit with an explicit session branch:

```sh
git --git-dir /cache/transport.git worktree add \
  -b weavie/fix/transport /worktrees/fix/transport LOCKED_COMMIT
git --git-dir /cache/transport.git worktree list --porcelain -z
```

Linked worktrees share repository data and have separate indexes and HEADs.
Git normally rejects a branch already checked out elsewhere and refuses dirty
worktree removal without force. Use distinct session branches and ordinary
absolute Git administrative links initially.
[Git worktree](https://git-scm.com/docs/git-worktree).

Use an ordinary bare cache with explicit fetch/session refs. Mirror updates can
overwrite mirrored refs. Partial clone reduces downloaded Git objects; sparse
checkout reduces visible working-tree files. These are separate optimizations.
Defer sparse checkout until its interaction with Nix source snapshots is tested.
[Clone and mirror behavior](https://git-scm.com/docs/git-clone),
[sparse checkout](https://git-scm.com/docs/git-sparse-checkout).

**Design decision:** serialize cache/worktree allocation, retain a ref to every
editable commit, and reject colliding aliases or paths outside the session.
Unlinking a workspace view does not discard its worktree. Removal checks dirty
state and unpushed commits. Do not add force-removal as routine cleanup.

## Two symlink views

An immutable Nix view can use `pkgs.linkFarm` to retain repository boundaries;
`symlinkJoin` merges directory contents and is suited to a different layout.
[Nixpkgs trivial builders](https://nixos.org/manual/nixpkgs/stable/#chap-trivial-builders).

```nix
pkgs.linkFarm "source-view" [
  { name = "apps/app"; path = inputs.app.outPath; }
  { name = "libs/transport"; path = inputs.transport.outPath; }
]
```

Editable views need links created outside the Nix store. A sibling symlink does
not declare a build dependency: sandboxed builds see declared store dependencies
and configured sandbox paths. Feed sources into the Nix expression explicitly.
[Nix sandbox](https://nix.dev/manual/nix/2.34/command-ref/conf-file#conf-sandbox).

Retained source snapshots need registered GC roots; arbitrary links in a session
are insufficient. `nix-store --realise STORE_PATH --add-root ABSOLUTE_ROOT`
provides a documented registration path. Remove only roots owned by the session.
[Root registration](https://nix.dev/manual/nix/2.32/command-ref/nix-store/realise).

Evaluate Nix definitions from source inputs. Importing expressions through a
built link-farm output introduces Import From Derivation, which can force builds
during evaluation and obstruct cheap planning.
[Import From Derivation](https://nix.dev/manual/nix/2.34/language/import-from-derivation).

## The hard boundary: source provenance

Inference from the mechanisms above: matching raw flake source paths to
derivation source inputs can identify some owners, but cannot provide complete
repository provenance. Filtering, vendoring, generated code, assembled sources,
and evaluation-only imports break a simple one-to-one match.

Ship a small opt-in Nix helper alongside the future CLI backend. Export provenance
from the same values used to build each package: original input instance,
transformed source, repository-relative directory, and owning derivation. A
custom `graphRepo.targets.<system>.<target>` output could carry it. This is a
proposed convention, not a built-in Nix API. Actual dependency edges still come
from derivations. For unadapted third-party packages, expose input-level data and
mark unresolved owners; never guess ownership from derivation names.

For Rust, declaring a Nix input does not make Cargo use it. Generate a session
patch configuration, apply it from the right Cargo root, and use the same sources
in the Nix build. Cargo `[patch]` resolution depends on source URLs, package
versions, and the lock; validate the resolved package graph.
[Cargo dependency overrides](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html).

Package-specific derivations and file sets can improve build reuse. Crane shows
shared dependency artifacts and per-crate source selection. Use this as an adapter
reference, rather than asserting Nix automatically turns a Cargo workspace into
fine-grained derivations.
[Crane workspace example](https://crane.dev/examples/quick-start-workspace.html).

## Existing projects

Logos Workspace is a close implementation precedent: downstream builds with
local input overrides, graph/impact inspection, and worktree workflows. Its
documentation includes explicit dependency metadata and a repository-wide
worktree workflow. weavie's proposed distinction is selecting source instances
from Nix and materializing just the requested subset.
[Logos Workspace](https://github.com/logos-co/logos-workspace).

devenv composes environments and supports cross-project configuration references.
Its polyrepo guide documents limits: imported projects' `devenv.yaml` is not
evaluated and profiles do not work with cross-project references. It is relevant
to services/processes, not a replacement for source-instance management.
[devenv polyrepos](https://devenv.sh/guides/polyrepo/).

## Implementation boundary and next execution test

0.1.0 implements `graph` and `why`, lock-format-7 parsing, `follows` resolution,
qualified selectors, reachable input selection, and deterministic JSON. It
accepts saved metadata and preserves fetcher attributes, non-flake inputs, and
unknown node attributes. It does not validate NAR hashes or verify that an input
can be fetched. It deliberately does not infer an output's build closure from
the lock graph.

The next milestone should prove this sequence with app, transport, protocol, and
an unrelated repository:

1. Inspecting the graph creates no member worktrees.
2. Opening app creates only explicitly selected worktrees and links.
3. Editing transport changes app's build through the session's override.
4. The unrelated repository stays absent from the workspace.
5. Two sessions can edit transport independently.
6. A retained store source can be promoted to an editable worktree.
7. Filtered-source provenance still identifies its owning repository.
8. Cargo and Nix consume the same selected sources.
9. GC preserves retained sources; dirty/unpushed worktrees survive release.
10. A fresh environment reproduces the committed composition and lock.

**Executed evidence on the initial machine:** `cargo test --workspace --locked`
passed 16 tests. `cargo clippy --workspace --all-targets --locked -- -D warnings`
passed. A temporary Git experiment used `clone --bare`, two `worktree add -b`
operations, and a source symlink. An edit reached only the selected worktree;
same-branch allocation and dirty `worktree remove` both failed with exit 128.
`worktree list --porcelain -z` listed one bare cache and two worktrees.

**Gap: live Nix workflow — no Nix executable on this machine.** Nix command
examples and integration design are checked against upstream documentation;
source realization, local override propagation, GC retention, sparse-worktree
snapshots, and Cargo/Nix parity remain unproven until the execution test runs.
