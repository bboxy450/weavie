{
  description = "weavie: Rust workspaces composed from Nix repository inputs";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      package = system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in pkgs.rustPlatform.buildRustPackage {
          pname = "weavie";
          version = "0.2.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.git ];
          cargoBuildFlags = [ "-p" "weavie" ];
          cargoTestFlags = [ "-p" "weavie" ];
        };
    in {
      packages = forAllSystems (system: { default = package system; weavie = package system; });
      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in { default = pkgs.mkShell { packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.git ]; }; });
      lib.mkRustGraph = import ./crates/weavie/nix/lib.nix;
    };
}
