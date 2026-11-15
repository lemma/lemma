{
  description = "Lemma dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustNightly = pkgs.rust-bin.nightly.latest.default;
          uniffiBindgenCs = pkgs.rustPlatform.buildRustPackage {
            pname = "uniffi-bindgen-cs";
            version = "0.11.0+v0.31.0";
            src = pkgs.fetchFromGitHub {
              owner = "NordSecurity";
              repo = "uniffi-bindgen-cs";
              rev = "v0.11.0+v0.31.0";
              hash = "sha256-vrT6t5YV4jH+434so/4bL5sTyZatCm1XvzZXExhAuXU=";
            };
            useFetchCargoVendor = true;
            cargoHash = "sha256-TcGTugnewS4QxO8KHM/2tBSOuYSz7cnKMQbl4RuRyc0=";
            buildAndTestSubdir = "bindgen";
            doCheck = false;
            meta.mainProgram = "uniffi-bindgen-cs";
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              rustToolchain
              rustNightly
              pkgs.rustup
              pkgs.cargo-nextest
              pkgs.cargo-deny
              pkgs.cargo-fuzz
              pkgs.cargo-llvm-cov
              pkgs.wasm-pack
              uniffiBindgenCs
              pkgs.nodejs_24
              pkgs.beamPackages.elixir
              pkgs.beamPackages.hex
              pkgs.beamPackages.rebar3
              pkgs.temurin-bin-26
              pkgs.dotnet-sdk_10
              pkgs.python3
              pkgs.git
              pkgs.pkg-config
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];

            JAVA_HOME = "${pkgs.temurin-bin-26}";
            # Apphost (e.g. csharpier) needs the real SDK root, not the wrapper.
            DOTNET_ROOT = "${pkgs.dotnet-sdk_10.unwrapped}/share/dotnet";

            # Force rustup to use the Nix-linked toolchains; do not download from rustup.rs.
            RUSTUP_TOOLCHAIN = "lemma";

            shellHook = ''
              export LC_ALL=en_US.UTF-8
              # Maven jar plugin rejects SOURCE_DATE_EPOCH=0 / 1980-01-01T00:00:00Z.
              unset SOURCE_DATE_EPOCH
              _lemma_cache="''${XDG_CACHE_HOME:-$HOME/.cache}"
              # Keep cargo plugins off ~/.cargo/bin (host cargo-deny etc.).
              export CARGO_HOME="$_lemma_cache/lemma-cargo"
              export RUSTUP_HOME="$_lemma_cache/lemma-rustup"
              mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
              unset _lemma_cache
              rustup toolchain link lemma "${rustToolchain}" >/dev/null
              # rustup reserves the name "nightly"; link as lemma-nightly for `rustup run`.
              rustup toolchain link lemma-nightly "${rustNightly}" >/dev/null
              rustup default lemma >/dev/null
            '';
          };
        }
      );

      formatter = forSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);
    };
}
