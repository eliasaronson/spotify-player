{
  description = "spotify-player flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        defaultPackage = pkgs.callPackage ./default.nix { };
        devShell =
          with pkgs;
          mkShell {
            buildInputs = [
              cargo
              pkg-config
              rustc

              # spotify-player dependencies
              alsa-lib
              libpulseaudio
              dbus-glib
              libsixel
              openssl
            ];

            # libpulse-binding links with `-l:libpulse.so.0`, for which the Nix linker wrapper
            # adds no rpath, so the binary couldn't find libpulse at runtime.
            RUSTFLAGS = "-C link-arg=-Wl,-rpath,${lib.makeLibraryPath [ libpulseaudio ]}";
          };
      }
    );
}
