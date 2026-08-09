{
  description = "Local-first macOS CLI for Apple Notes, Reminders, Calendar, and Messages";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      lib = nixpkgs.lib;
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems = lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "apple-cli";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            meta = {
              description = "AppleScript-powered CLI for macOS Notes, Reminders, Calendar, and Messages";
              homepage = "https://github.com/Sankalpcreat/Apple-CLI";
              license = lib.licenses.mit;
              mainProgram = "apple";
              platforms = lib.platforms.darwin;
            };
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clang
              clippy
              python3
              rustc
              rustfmt
            ];
          };
        }
      );
    };
}
