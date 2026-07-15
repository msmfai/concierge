{
  description = "Concierge — declarative Bethesda mod management (Fallout 4 prototype, CrossOver/macOS)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      mkConcierge = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "concierge";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "concierge" ];
        cargoTestFlags = [ "-p" "concierge" ];
        # runtime deps: archive extraction shells out to bsdtar (libarchive);
        # the metadata store shells out to `clickhouse local`
        nativeBuildInputs = [ pkgs.makeWrapper ];
        postInstall = ''
          wrapProgram $out/bin/concierge --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.libarchive pkgs.clickhouse ]}
        '';
      };
    in
    {
      packages = forAll (pkgs: rec {
        concierge = mkConcierge pkgs;
        default = concierge;
      });

      apps = forAll (pkgs:
        let tool = mkConcierge pkgs; in rec {
          concierge = { type = "app"; program = "${tool}/bin/concierge"; };
          default = concierge;
        });

      # GUI (eframe): build/run in the devshell with `cargo run -p concierge-gui`.
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.libarchive
            pkgs.clickhouse
          ];
        };
      });
    };
}
