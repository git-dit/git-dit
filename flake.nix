{
  description = "The git-dit project";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-utils.lib.eachSystem [ "x86_64-linux" ] (
      system:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          overlays =
            let
              selfOverlay = _: _: { } // inputs.self.packages."${system}";
            in
            [
              selfOverlay
              (import inputs.rust-overlay)
            ];
        };

        rustTarget = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustTarget;

        tomlInfo = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };

        inherit (tomlInfo) version;
        pname = "git-dit";

        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = pkgs.lib.cleanSourceFilter;
        };

        gitditBuildInputs = [
        ];

        gitditNativeBuildInputs = [
        ];

        cargoArtifacts = craneLib.buildDepsOnly {
          inherit src pname;

          buildInputs = gitditBuildInputs;
          nativeBuildInputs = gitditNativeBuildInputs;
        };

        gitdit = craneLib.buildPackage {
          inherit
            cargoArtifacts
            src
            pname
            version
            ;

          cargoExtraArgs = "--all-features -p gitdit";

          buildInputs = gitditBuildInputs;
          nativeBuildInputs = gitditNativeBuildInputs;
        };

        gitdit-docs = craneLib.cargoDoc {
          inherit
            cargoArtifacts
            src
            pname
            version
            ;

          doInstallCargoArtifacts = true;

          RUSTDOCFLAGS = "-D warnings"; # Error out if there is a warning
        };

        gitdit-doctests = craneLib.cargoTest {
          inherit
            cargoArtifacts
            src
            pname
            version
            ;

          cargoExtraArgs = "--doc";

          buildInputs = gitditBuildInputs;
          nativeBuildInputs = gitditNativeBuildInputs;
        };
      in
      rec {
        checks = {
          inherit gitdit;
          inherit gitdit-doctests;
          inherit gitdit-docs;

          gitdit-clippy = craneLib.cargoClippy {
            inherit cargoArtifacts src pname;

            cargoClippyExtraArgs = "--benches --examples --tests --all-features -- --deny warnings";
          };

          gitdit-clippy-no-features = craneLib.cargoClippy {
            inherit cargoArtifacts src pname;

            cargoClippyExtraArgs = "--benches --examples --tests --no-default-features -- --deny warnings";
          };

          gitdit-fmt = craneLib.cargoFmt {
            inherit src pname;
          };

          gitdit-tests = craneLib.cargoNextest {
            inherit cargoArtifacts src pname;

            buildInputs = gitditBuildInputs;
            nativeBuildInputs = [
              pkgs.coreutils
            ] ++ gitditNativeBuildInputs;
          };
        };

        packages = {
          default = packages.gitdit;
          inherit gitdit;
          inherit gitdit-docs;
        };

        apps.git-dit = inputs.flake-utils.lib.mkApp {
          drv = packages.gitdit;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = gitditBuildInputs;

          nativeBuildInputs = gitditNativeBuildInputs ++ [
            rustTarget

            pkgs.gitlint
            pkgs.statix
          ];
        };
      }
    );
}
