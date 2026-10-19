# weavie

An offline Nix input-graph inspector, and the foundation for selective
multi-repository workspaces built from Nix inputs, Git worktrees, and symlinks.

```sh
cargo install weavie --locked

# Inside a composition with an existing flake.lock:
weavie graph
weavie graph --from app --json
weavie why protocol --from app
weavie why app/transport
weavie --file /path/to/flake.lock graph

# Accept saved Nix metadata or flake.lock JSON on stdin:
nix flake metadata --json --no-update-lock-file github:owner/composition \
  | weavie --file - graph --json
```

Input selectors use slash-separated input names relative to the lock root;
`.` selects the root. `why` explains a shortest declared input path from the
chosen starting input. Shared aliases retain their original `follows` paths.
Identical repositories at different lock nodes remain separate instances.

Version 0.1.0 supports lock format 7 and handles circular input graphs. It
rejects dangling references, alias cycles that never resolve, and unsupported
lock versions. JSON has `schema_version: 1`, sorted node/input keys, resolved
edges, original fetcher attributes, and `lock_root` for the original `follows`
namespace. A selected graph's `root` can differ from `lock_root`.

The CLI reads local files or stdin. It does not fetch, clone, create worktrees,
mount repositories, or invoke Nix. The example Nix command can fetch sources.
Source-input edges are not a selected package's build dependencies.

The library exposes `Graph::from_json`, `resolve_input`, `select`, and `explain`.
The [project research and proposed workflow](https://github.com/bboxy450/weavie/blob/main/docs/nix-graph-repo.md)
cover target provenance and future worktree/build integration.

MIT licensed.
