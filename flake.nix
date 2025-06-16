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
          fs = inputs.nixpkgs.lib.fileset;
          src = fs.toSource {
            root = ./.;
            fileset = fs.unions [
              ./lib
              ./src
              ./tests
              ./Cargo.lock
              ./Cargo.toml
            ];
          };
          # Use a fixed fake sha1 and timestamp to make the build cacheable.
          fakeTimestamp = "-NO_TIMESTAMP-";
          fakeRev =   "---------------NO_VERSION---------------";
          git-toprepo-unpatched = rustPlatform.buildRustPackage {
              inherit src;
              name = "git-toprepo";
              BUILD_SCM_TAG = "nix";
              BUILD_SCM_TIMESTAMP = fakeTimestamp;
              BUILD_SCM_REVISION = fakeRev;
              nativeBuildInputs = with pkgs; [
                  git
              ];
              cargoLock.lockFile = "${src}/Cargo.lock";
              passthru = {
                inherit (self) lastModifiedDate;
                rev = if (self ? rev) then self.rev else self.dirtyRev;
              };
            };
          git-toprepo-patched = let
              inherit (git-toprepo-unpatched.passthru) rev lastModifiedDate;
            in pkgs.runCommand "git-toprepo-patched" {
                  PATH = pkgs.lib.strings.makeSearchPath "bin" [
                    pkgs.gnused
                  ];
                } ''
              mkdir -p $out/bin
              newRev="$(printf "${rev}" | sed 's/^\(.\{34\}\).*-dirty$/\1-dirty/')"
              sed \
                -e "s/${fakeRev}/$newRev/" \
                -e "s/${fakeTimestamp}/${lastModifiedDate}/" \
              ${git-toprepo-unpatched}/bin/git-toprepo \
              > $out/bin/git-toprepo
              chmod +x $out/bin/git-toprepo
            '';
        in {
          packages = {
            inherit git-toprepo-unpatched;
            git-toprepo = git-toprepo-patched;
            default =  git-toprepo-patched;
          };
          devShells.default = pkgs.mkShell {
            buildInputs = [
              toolchain
            ];
          };
          apps = let
              git-toprepo-app = {
                type = "app";
                program = "${git-toprepo-patched}/bin/git-toprepo";
              };
            in {
              default = git-toprepo-app;
              git-toprepo = git-toprepo-app;
            };
        };
    };
}
