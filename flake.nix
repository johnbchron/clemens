{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    rust-overlay = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "https://flakehub.com/f/oxalica/rust-overlay/0.1.tar.gz";
    };
    crane = {
      url = "https://flakehub.com/f/ipetkov/crane/0.16.tar.gz";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, crane, flake-utils }: 
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        });

        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

        common_args = {
          src = craneLib.cleanCargoSource (craneLib.path ./.);
          doCheck = false;

          buildInputs = [ ];
          nativeBuildInputs = with pkgs; [ pkg-config cmake alsa-lib rustPlatform.bindgenHook makeWrapper ];
        };

        deps_only = craneLib.buildDepsOnly common_args;
        crate = craneLib.buildPackage (common_args // {
          cargoArtifacts = deps_only;
        });
      in {
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            bacon
            toolchain
          ] ++ common_args.nativeBuildInputs ++ common_args.buildInputs;
        };
        packages = {
          default = crate;
        };
      });
}
