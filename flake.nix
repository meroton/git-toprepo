{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = {self, flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      perSystem = { config, pkgs, inputs', ... }: let
        toolchain = inputs'.fenix.packages.fromToolchainFile {
          file = builtins.path {
            name = "rust-toolchain";
            path = ./rust-toolchain.toml;
          };
          sha256 = "sha256-pw28Lw1M3clAtMjkE/wry0WopX0qvzxeKaPUFoupC00=";
        };
        rustPlatform = pkgs.makeRustPlatform {
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
        # Use a fixed sha1 and timestamp to make the build cacheable.
        # invalid timestamp that we assumen won't be present in the binary
        # unless it's set by the `BUILD_SCM_TIMESTAMP`
        fakeTimestamp = "19691332246060";
        # sha1 generated with 'printf "git-toprepo" | sha1sum'
        fakeRev = "a47761350505d860d99bac0bed8e02303874b689";
        git-toprepo-cargo-bin = rustPlatform.buildRustPackage {
            inherit src;
            name = "git-toprepo";
            BUILD_SCM_TAG = "nix";
            BUILD_SCM_TIMESTAMP = fakeTimestamp;
            BUILD_SCM_REVISION = fakeRev;
            nativeBuildInputs = with pkgs; [
                git
            ];
            cargoLock.lockFile = "${src}/Cargo.lock";
          };
        git-toprepo-patched = let
            revision = if (self ? rev) then self.rev else self.dirtyRev;
            timestamp = self.lastModifiedDate;
          in pkgs.runCommand "git-toprepo-patched" {
                PATH = pkgs.lib.strings.makeSearchPath "bin" [
                  pkgs.gnused
                ];
              } ''
            mkdir -p $out/bin
            newRev="$(printf "${revision}" | sed 's/^\(.\{34\}\).*-dirty$/\1-dirty/')"
            sed \
              -e "s/${fakeRev}/$newRev/" \
              -e "s/${fakeTimestamp}/${timestamp}/" \
            ${git-toprepo-cargo-bin}/bin/git-toprepo \
            > $out/bin/git-toprepo
            chmod +x $out/bin/git-toprepo
          '';
        in git-toprepo-patched;
        in {
          packages = {
            inherit git-toprepo;
            default =  git-toprepo;
          };
          devShells.default = pkgs.mkShell {
            buildInputs = [
              toolchain
            ];
          };
          apps = let
              git-toprepo-app = {
                type = "app";
                program = "${git-toprepo}/bin/git-toprepo";
              };
            in {
              default = git-toprepo-app;
              git-toprepo = git-toprepo-app;
            };
        };
    };
}
