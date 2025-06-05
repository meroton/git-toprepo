{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = {flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      perSystem = { config, pkgs, inputs', ... }: let
        rustPlatform = let
          toolchain = inputs'.fenix.packages.minimal.toolchain;
        in pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
        };
        git-toprepo = let
          # Needed to make the source path reproducible:
          # https://nix.dev/guides/best-practices#reproducible-source-paths
          src = builtins.path { path = ./.; name = "git-toprepo"; };
        in rustPlatform.buildRustPackage {
          inherit src;
          pname = "git-toprepo";
          version = "0.0.0";  # TODO: Use git-hash or similar.
          cargoLock.lockFile = "${src}/Cargo.lock";
          doCheck = false;  # TODO(albinvass): Fix tests in nix sandbox
        };
        in {
          packages = {
            inherit git-toprepo;
            default =  git-toprepo;
          };
          apps = let
              git-toprepo-app = {
                type = "app";
                program = "${git-toprepo}/bin/git-toprepo";
              };
            in {
              inherit git-toprepo;
              default = git-toprepo-app;
            };
        };
    };
}
