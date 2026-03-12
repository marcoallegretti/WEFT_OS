{
  description = "WEFT OS — capability-secure Wayland compositor and app runtime";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
    in
    flake-utils.lib.eachSystem supportedSystems (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rust193 = pkgs.rust-bin.stable."1.93.0".default;
        weftPkgs = pkgs.callPackage ./infra/nixos/weft-packages.nix {
          inherit rust193;
        };
      in
      {
        packages = weftPkgs // {
          default = weftPkgs.weft-appd;
        };

        devShells.default = pkgs.mkShell {
          name = "weft-dev";
          nativeBuildInputs = with pkgs; [
            rust193
            pkg-config
            cmake
            clang
            python3
          ];
          buildInputs = with pkgs; [
            openssl
            libdrm
            mesa
            wayland
            wayland-protocols
            libxkbcommon
            libseat
            udev
            dbus
            libGL
          ];
          shellHook = ''
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
              pkgs.mesa pkgs.wayland pkgs.libxkbcommon pkgs.libdrm
            ]}:$LD_LIBRARY_PATH"
          '';
        };
      }
    ) // {
      nixosConfigurations.weft-vm = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = {
          inherit self;
          rustOverlay = rust-overlay.overlays.default;
        };
        modules = [
          ./infra/nixos/configuration.nix
        ];
      };
    };
}
