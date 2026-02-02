{
  description = "A logo discovery tool that crawls domains.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
  in {
    packages = forAllSystems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [rust-overlay.overlays.default];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in {
      default = pkgs.rustPlatform.buildRustPackage {
        pname = cargoToml.package.name;
        version = cargoToml.package.version;
        description = cargoToml.pacakge.description;
        license = cargoToml.package.license;
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [rustToolchain];
      };
    });

    devShells = forAllSystems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [rust-overlay.overlays.default];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      isLinux = pkgs.lib.hasInfix "linux" system;
    in {
      default = pkgs.mkShell {
        buildInputs =
          [
            rustToolchain
            pkgs.rust-analyzer
            pkgs.just
          ]
          ++ pkgs.lib.optionals isLinux [
            pkgs.lldb
          ];

        RUST_BACKTRACE = "1";
      };
    });
  };
}
