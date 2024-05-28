{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    poetry2nix = {
      url = "github:nix-community/poetry2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = {flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" ];
      perSystem = { config, pkgs, inputs', ... }: let
          mkPoetryApplication = (inputs.poetry2nix.lib.mkPoetry2Nix {
            inherit pkgs;
          }).mkPoetryApplication;
          git-toprepo = mkPoetryApplication {
            projectDir = ./.;
            overrides = final: prev: {
              packaging = prev.packaging.overridePythonAttrs (old: {
                buildInputs = (old.buildInputs or []) ++ [prev.flit-core];
              });
              git-filter-repo = prev.git-filter-repo.overridePythonAttrs (old: {
                buildInputs = (old.buildInputs or []) ++ [prev.setuptools prev.flit-core];
                postPatch = ''
                  # fix: ERROR: Could not find a version that satisfies the requirement setuptools_scm
                  substituteInPlace setup.py \
                    --replace \
                      'use_scm_version=dict(root="..", relative_to=__file__),' \
                      'version="2.38.0",'
                  substituteInPlace setup.cfg \
                    --replace 'setup_requires = setuptools_scm' ""

                  # fix: FileExistsError: File already exists: /bin/git-filter-repo
                  substituteInPlace setup.cfg \
                    --replace "scripts = git-filter-repo" ""
                '';
              });
            };
          };
        in {
          packages.git-toprepo =  git-toprepo;
          apps.git-toprepo = {
            type = "app";
            program = "${git-toprepo}/bin/git-toprepo";
          };
        };
    };
}
