{ pkgs, ... }:

let
  src = ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = {
      "servo-0.0.1" = pkgs.lib.fakeSha256;
    };
  };

  commonArgs = {
    inherit src cargoLock;
    version = "0.1.0";
    nativeBuildInputs = with pkgs; [ pkg-config ];
  };

  mkWeftPkg = { pname, extraBuildInputs ? [], extraNativeBuildInputs ? [], cargoFlags ? [] }: pkgs.rustPlatform.buildRustPackage (commonArgs // {
    inherit pname;
    cargoBuildFlags = [ "--package" pname ] ++ cargoFlags;
    cargoTestFlags = [ "--package" pname ];
    buildInputs = extraBuildInputs;
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ extraNativeBuildInputs;
    doCheck = true;
  });

in {
  weft-compositor = mkWeftPkg {
    pname = "weft-compositor";
    extraBuildInputs = with pkgs; [
      libdrm
      mesa
      wayland
      libxkbcommon
      libseat
      udev
      dbus
      libGL
    ];
    extraNativeBuildInputs = with pkgs; [ wayland-scanner ];
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
