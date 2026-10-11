{ pkgs, lib, rust193, ... }:

let
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rust193;
    rustc = rust193;
  };

  # The workspace without build outputs, so a local `target/` never enters
  # the store.
  src = lib.cleanSourceWith {
    src = lib.cleanSource ../..;
    filter = path: _type:
      let rel = lib.removePrefix (toString ../.. + "/") (toString path);
      in !(lib.hasPrefix "target" rel || lib.hasSuffix "-result" rel);
  };

  servoHash = "sha256-L5XlHf5GOoPtyitRsbb+keXSkTMSJWmYCrWCHzGFI5M=";

  # The pinned Servo revision as a whole tree; see servo-tree.py.
  servoSrc = pkgs.fetchgit {
    url = "https://github.com/marcoallegretti/servo";
    rev = "e5d9a518ebddb5ebd6af404d1fd4e00ea0f77163";
    hash = servoHash;
  };

  cargoLock = {
    lockFile = ../../Cargo.lock;
    # One hash per git source in Cargo.lock: the Servo fork and the Stylo fork
    # (keyed by its `selectors` crate). Recompute them whenever a revision
    # changes; a build with a stale hash reports the right one.
    outputHashes = {
      "servo-0.0.1" = servoHash;
      "selectors-0.36.0" = "13y6pa19j4w2zsxfhv6mrl03wsa0frazlsssbqkzpwci8k9gzl4i";
    };
  };

  # SpiderMonkey, prebuilt by the mozjs project for the mozjs_sys version in
  # Cargo.lock. The build has no network, and compiling SpiderMonkey from
  # source would add hours and gigabytes to every build.
  mozjsArchive = pkgs.fetchurl {
    url = "https://github.com/servo/mozjs/releases/download/mozjs-sys-v0.140.8-0/libmozjs-x86_64-unknown-linux-gnu.tar.gz";
    hash = "sha256-wEDh2VCawPLc7AOlaitkT5yN9pv16oX7UoeHMg+4Aeg=";
  };

  # Libraries Servo and winit open at run time rather than link against.
  runtimeLibs = with pkgs; [ libGL wayland libxkbcommon vulkan-loader ];

  packages = [
    "weft-compositor"
    "weft-servo-shell"
    "weft-app-shell"
    "weft-appd"
    "weft-runtime"
    "weft-pack"
    "weft-file-portal"
    "weft-mount-helper"
  ];

  features = [
    "weft-servo-shell/servo-embed"
    "weft-app-shell/servo-embed"
    "weft-runtime/wasmtime-runtime"
    "weft-runtime/net-fetch"
  ];

  # Every binary comes from one cargo build, so the dependencies the
  # programs share, Servo above all, compile once.
  workspace = rustPlatform.buildRustPackage {
    pname = "weft";
    version = "0.0.1-alpha.0";
    inherit src cargoLock;

    cargoBuildFlags = lib.concatMap (p: [ "--package" p ]) packages
      ++ [ "--features" (lib.concatStringsSep "," features) ];
    doCheck = false;

    nativeBuildInputs = with pkgs; [
      pkg-config
      wayland-scanner
      cmake
      python3
      perl
      m4
      rustPlatform.bindgenHook
    ];

    buildInputs = with pkgs; [
      libdrm mesa libgbm wayland libxkbcommon seatd udev dbus libGL
      libdisplay-info libinput
      openssl cryptsetup
      fontconfig freetype libunwind
    ];

    env = {
      MOZJS_ARCHIVE = "${mozjsArchive}";
      RUSTFLAGS = "-L${pkgs.libgbm}/lib -L${pkgs.libinput}/lib";
      # The helper runs setuid and takes no program from its caller's PATH.
      WEFT_VERITYSETUP = "${pkgs.cryptsetup}/bin/veritysetup";
    };

    # Servo's code generator finds its WebIDL parser from its own path
    # without resolving links, so the tree's third_party/ is also linked
    # where that path leads from the vendor directory.
    preBuild = ''
      python3 ${./servo-tree.py} ${servoSrc} Cargo.lock "$NIX_BUILD_TOP/cargo-vendor-dir" "$NIX_BUILD_TOP/servo"
      ln -s "$NIX_BUILD_TOP/servo/third_party" "$NIX_BUILD_TOP/third_party"
    '';

    # The system UI page and the UI kit both shells inject, at the paths the
    # session's units name.
    postInstall = ''
      install -Dm644 infra/shell/system-ui.html $out/share/weft/shell/system-ui.html
      install -Dm644 infra/shell/weft-ui-kit.js $out/share/weft/system/weft-ui-kit.js
    '';

    postFixup = ''
      for bin in weft-servo-shell weft-app-shell weft-compositor; do
        patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/$bin
      done
    '';
  };

in
# Each program keeps its own attribute for the units that name it.
lib.genAttrs packages (_: workspace) // { weft = workspace; }
