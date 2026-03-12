{ pkgs, rust193, ... }:

let
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rust193;
    rustc = rust193;
  };

  src = ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = {
      "servo-0.0.1" = "0b803qankr0rs4hi0md26dydf2cvpd6v5x2bxxypzsga0jwfdd26";
      "selectors-0.36.0" = "1x5g61cadq700yhl1wwrjd043grlpdviqqn4n9cm5k68gbx0if81";
    };
  };

  commonArgs = {
    inherit src cargoLock;
    version = "0.1.0";
    nativeBuildInputs = with pkgs; [ pkg-config ];
  };

  mkWeftPkg = { pname, extraBuildInputs ? [], extraNativeBuildInputs ? [], cargoFlags ? [], extraEnv ? {} }: rustPlatform.buildRustPackage (commonArgs // {
    inherit pname;
    cargoBuildFlags = [ "--package" pname ] ++ cargoFlags;
    cargoTestFlags = [ "--package" pname ];
    buildInputs = extraBuildInputs;
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ extraNativeBuildInputs;
    env = extraEnv;
    doCheck = false;
  });

in {
  weft-compositor = mkWeftPkg {
    pname = "weft-compositor";
    extraBuildInputs = with pkgs; [
      libdrm mesa wayland libxkbcommon seatd udev dbus libGL
    ];
    extraNativeBuildInputs = with pkgs; [ wayland-scanner ];
  };

  weft-servo-shell = mkWeftPkg {
    pname = "weft-servo-shell";
    extraBuildInputs = with pkgs; [
      mesa wayland libxkbcommon openssl dbus udev libGL fontconfig
    ];
    extraNativeBuildInputs = with pkgs; [
      pkgs.llvmPackages.clang cmake python3
    ];
    cargoFlags = [ "--features" "servo-embed" ];
    extraEnv = {
      LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    };
  };

  weft-app-shell = mkWeftPkg {
    pname = "weft-app-shell";
    extraBuildInputs = with pkgs; [
      mesa wayland libxkbcommon openssl dbus udev libGL fontconfig
    ];
    extraNativeBuildInputs = with pkgs; [
      pkgs.llvmPackages.clang cmake python3
    ];
    cargoFlags = [ "--features" "servo-embed" ];
    extraEnv = {
      LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    };
  };

  weft-appd = mkWeftPkg {
    pname = "weft-appd";
    extraBuildInputs = with pkgs; [ openssl ];
  };

  weft-runtime = mkWeftPkg {
    pname = "weft-runtime";
    extraBuildInputs = with pkgs; [ openssl ];
    cargoFlags = [ "--features" "wasmtime-runtime,net-fetch" ];
  };

  weft-pack = mkWeftPkg {
    pname = "weft-pack";
  };

  weft-file-portal = mkWeftPkg {
    pname = "weft-file-portal";
  };

  weft-mount-helper = mkWeftPkg {
    pname = "weft-mount-helper";
    extraBuildInputs = with pkgs; [ cryptsetup ];
  };
}
