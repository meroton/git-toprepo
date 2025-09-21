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
      imports = [
        inputs.flake-parts.flakeModules.easyOverlay
      ];
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
          # Need to manually update the hash to get the correct toolchain. It
          # does not re-evaluate just on changed rust-toolchain.toml.
          # curl https://static.rust-lang.org/dist/2025-09-20/channel-rust-nightly.toml |
          #   sha256sum | cut -f1 -d' ' | xxd -r -p | base64
          sha256 = "sha256-qvgL9thRqOhiZX1xdkm4TlOm7TTEglHT6NumEJJWzdc=";
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
          git-toprepo = rustPlatform.buildRustPackage {
              inherit src;
              name = "git-toprepo";
              BUILD_SCM_TAG = "nix";
              BUILD_SCM_TIMESTAMP = fakeTimestamp;
              BUILD_SCM_REVISION = fakeRev;
              nativeBuildInputs = with pkgs; [
                  git
                  python3
              ];
              cargoLock.lockFile = "${src}/Cargo.lock";
              passthru = {
                inherit (self) lastModifiedDate;
                rev = if (self ? rev) then self.rev else self.dirtyRev;
              };
            };
          git-toprepo-stamped = let
              inherit (git-toprepo.passthru) rev lastModifiedDate;
            in pkgs.runCommand "git-toprepo-stamped" {
                  PATH = pkgs.lib.strings.makeSearchPath "bin" [
                    pkgs.gnused
                  ];
                } ''
              mkdir -p $out/bin
              newRev="$(printf "${rev}" | sed 's/^\(.\{34\}\).*-dirty$/\1-dirty/')"
              sed \
                -e "s/${fakeRev}/$newRev/" \
                -e "s/${fakeTimestamp}/${lastModifiedDate}/" \
              ${git-toprepo}/bin/git-toprepo \
              > $out/bin/git-toprepo
              chmod +x $out/bin/git-toprepo
            '';
        in {
          overlayAttrs = {
            inherit (config.packages) git-toprepo-stamped git-toprepo;
          };
          packages = {
            inherit git-toprepo-stamped git-toprepo;
            default =  git-toprepo-stamped;
          };
          devShells.default = pkgs.mkShell {
            buildInputs = [
              toolchain
            ];
          };
          apps = let
              git-toprepo-app = {
                type = "app";
                program = "${git-toprepo-stamped}/bin/git-toprepo";
              };
            in {
              default = git-toprepo-app;
              git-toprepo = git-toprepo-app;
            };
        };
    };
}
