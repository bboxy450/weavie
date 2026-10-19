# Imported by a composition: (import ./weavie.nix) { inherit pkgs inputs; ... }
{
  pkgs,
  inputs,
  repositories,
  rootRepository,
  cargoLock,
  cargoRoot ? ".",
  pname ? rootRepository,
  version ? "0.1.0",
  buildArgs ? { },
}:
let
  inherit (pkgs) lib;
  safePath = path:
    builtins.isString path && path != "" && !(lib.hasPrefix "/" path)
    && lib.all (part: part != ".." && part != "") (lib.splitString "/" path);
  resolve = input:
    if input == "self" then inputs.self
    else lib.foldl' (value: name: value.${name}) inputs (lib.splitString "/" input);
  normalized = lib.mapAttrs (alias: repo:
    assert builtins.match "[a-zA-Z0-9_-]+" alias != null;
    assert safePath (repo.input or alias);
    assert lib.all safePath (builtins.attrValues repo.crates);
    assert lib.all (name:
      (builtins.fromTOML (builtins.readFile "${resolve (repo.input or alias)}/${repo.crates.${name}}/Cargo.toml")).package.name == name
    ) (builtins.attrNames repo.crates);
    {
      input = repo.input or alias;
      source = resolve (repo.input or alias);
      crates = repo.crates;
      patchSources = repo.patchSources or [ "crates-io" ];
    }
  ) repositories;
  patchKeys = lib.concatMap (repo: lib.concatMap
    (source: map (name: builtins.toJSON [ source name ]) (builtins.attrNames repo.crates))
    repo.patchSources) (builtins.attrValues normalized);
  metadata = {
    schemaVersion = 1;
    inherit rootRepository cargoRoot;
    repositories = lib.mapAttrs (_: repo: {
      inherit (repo) input crates patchSources;
      storePath = toString repo.source;
    }) normalized;
  };
  patches = lib.foldlAttrs (patches: alias: repo:
    lib.foldl' (patches: source:
      patches // { ${source} = (patches.${source} or { }) //
        lib.mapAttrs (name: directory: {
          path = "@weavieRoot@/${alias}/${directory}";
        }) repo.crates;
      }
    ) patches repo.patchSources
  ) { } normalized;
  rootConfig = "${normalized.${rootRepository}.source}/${cargoRoot}/.cargo/config.toml";
  legacyConfig = "${normalized.${rootRepository}.source}/${cargoRoot}/.cargo/config";
  existingConfig = if builtins.pathExists rootConfig then builtins.fromTOML (builtins.readFile rootConfig)
    else if builtins.pathExists legacyConfig then builtins.fromTOML (builtins.readFile legacyConfig)
    else { };
  patchConfig = (pkgs.formats.toml { }).generate "weavie-cargo-config.toml"
    (lib.recursiveUpdate existingConfig { patch = patches; });
  source = pkgs.runCommand "${pname}-graph-source" { } (
    ''mkdir -p "$out"
    '' + lib.concatMapAttrsStringSep "\n" (alias: repo: ''
      cp -R --no-preserve=mode ${repo.source}/ "$out/${alias}"
    '') normalized + ''
      mkdir -p "$out/${rootRepository}/${cargoRoot}/.cargo"
      rm -f "$out/${rootRepository}/${cargoRoot}/.cargo/config"
      substitute ${patchConfig} "$out/${rootRepository}/${cargoRoot}/.cargo/config.toml" --replace-fail '@weavieRoot@' "$out"
      cp ${cargoLock} "$out/${rootRepository}/${cargoRoot}/Cargo.lock"
    ''
  );
  package = pkgs.rustPlatform.buildRustPackage (buildArgs // {
    inherit pname version;
    src = source;
    cargoLock = (buildArgs.cargoLock or { }) // { lockFile = cargoLock; };
    postUnpack = (buildArgs.postUnpack or "") + "\n" + ''
      sourceRoot="$sourceRoot/${rootRepository}/${cargoRoot}"
    '';
    nativeBuildInputs = (buildArgs.nativeBuildInputs or [ ]) ++ [ pkgs.jq ];
    preBuild = (buildArgs.preBuild or "") + "\n" + ''
      cargo metadata --offline --locked --format-version 1 > "$TMPDIR/weavie-metadata.json"
      ${pkgs.jq}/bin/jq -e --argjson selected '${builtins.toJSON (lib.concatMap (repo: builtins.attrNames repo.crates) (builtins.attrValues normalized))}' \
        '[.packages[] | select(.name as $name | $selected | index($name)) | .source == null] | all' \
        "$TMPDIR/weavie-metadata.json" >/dev/null || {
          echo 'weavie: Cargo resolved a selected crate outside the local source graph' >&2
          exit 1
        }
    '';
  });
in
assert builtins.hasAttr rootRepository normalized;
assert safePath cargoRoot;
assert builtins.length patchKeys == builtins.length (lib.unique patchKeys);
{
  inherit metadata package;
  devShell = pkgs.mkShell {
    inputsFrom = [ package ];
    packages = [ pkgs.cargo pkgs.rustc pkgs.git ];
  };
}
