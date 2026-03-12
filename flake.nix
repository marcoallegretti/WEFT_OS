{
  description = "WEFT OS — capability-secure Wayland compositor and app runtime";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
    in
    flake-utils.lib.eachSystem supportedSystems (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        weftPkgs = pkgs.callPackage ./infra/nixos/weft-packages.nix { };
      in
      {
        packages = weftPkgs // {
          default = weftPkgs.weft-appd;
        };

        devShells.default = pkgs.mkShell {
          name = "weft-dev";
          nativeBuildInputs = with pkgs; [
            rustup
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
        specialArgs = { inherit self; };
        modules = [
          ./infra/nixos/configuration.nix
        ];
      };
    };
}
