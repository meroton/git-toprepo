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
          toolchain = inputs'.fenix.packages.fromToolchainFile {
            file = builtins.path {
              name = "rust-toolchain";
              path = ./rust-toolchain.toml;
            };
            sha256 = "sha256-pw28Lw1M3clAtMjkE/wry0WopX0qvzxeKaPUFoupC00=";
          };
        in pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
        };
        git-toprepo = let
          fs = inputs.nixpkgs.lib.fileset;
          src = fs.toSource {
            root = ./.;
            fileset = fs.unions [
              ./src
              ./tests
              ./Cargo.lock
              ./Cargo.toml
            ];
          };
        in rustPlatform.buildRustPackage {
          inherit src;
          name = "git-toprepo";
          nativeBuildInputs = with pkgs; [
              git
          ];
          cargoLock.lockFile = "${src}/Cargo.lock";
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
