{ pkgs, rust193, ... }:

let
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rust193;
    rustc = rust193;
  };

  src = ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    # One hash per git source in Cargo.lock: the Servo fork and the Stylo fork
    # (keyed by its `selectors` crate). Every package vendors both, so both must
    # match the pinned revisions. Recompute them whenever a revision changes.
    # The Servo hash is not yet known for e5d9a51; a Nix build reports it.
    outputHashes = {
      "servo-0.0.1" = pkgs.lib.fakeHash;
      "selectors-0.36.0" = "13y6pa19j4w2zsxfhv6mrl03wsa0frazlsssbqkzpwci8k9gzl4i";
    };
  };

  commonArgs = {
    inherit src cargoLock;
    version = "0.1.0";
    nativeBuildInputs = with pkgs; [ pkg-config ];
  };

  mkWeftPkg = { pname, extraBuildInputs ? [], extraNativeBuildInputs ? [], cargoFlags ? [], extraEnv ? {}, preBuild ? "" }: rustPlatform.buildRustPackage (commonArgs // {
    inherit pname preBuild;
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
      libdrm mesa libgbm wayland libxkbcommon seatd udev dbus libGL libdisplay-info libinput
    ];
    extraNativeBuildInputs = with pkgs; [ wayland-scanner ];
    extraEnv = {
      RUSTFLAGS = "-L${pkgs.libgbm}/lib -L${pkgs.libinput}/lib";
    };
  };

  weft-servo-shell = mkWeftPkg {
    pname = "weft-servo-shell";
  };

  weft-app-shell = mkWeftPkg {
    pname = "weft-app-shell";
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
