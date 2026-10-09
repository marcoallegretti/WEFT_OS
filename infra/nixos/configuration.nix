{ pkgs, lib, self, modulesPath, rustOverlay, ... }:

{
  imports = [
    "${modulesPath}/profiles/qemu-guest.nix"
    "${modulesPath}/virtualisation/qemu-vm.nix"
  ];

  system.stateVersion = "25.05";

  boot.loader.grub = {
    enable = true;
    device = "/dev/vda";
  };

  fileSystems."/" = {
    device = "/dev/vda1";
    fsType = "ext4";
  };

  # The session is graphical: tty0 is the primary console, and the serial
  # console stays available as a secondary one for debugging.
  virtualisation = {
    graphics = true;
    memorySize = 4096;
    cores = 4;
    diskSize = 20480;
  };

  hardware.graphics = {
    enable = true;
    extraPackages = with pkgs; [ mesa.drivers virglrenderer ];
  };

  networking = {
    hostName = "weft-vm";
    firewall.enable = false;
  };

  time.timeZone = "UTC";

  users.users.weft = {
    isNormalUser = true;
    description = "WEFT OS session user";
    extraGroups = [ "video" "render" "seat" "input" "audio" ];
    password = "";
    autoSubUidGidRange = false;
  };

  services.getty.autologinUser = "weft";

  security.polkit.enable = true;

  # weft-mount-helper mounts verified package images and must run as root;
  # the Nix store cannot hold set-user-ID programs. Only the session user's
  # group may run it, and it mounts only in the caller's own runtime
  # directory (see crates/weft-mount-helper).
  security.wrappers.weft-mount-helper = {
    source = "${pkgs.weft.weft-mount-helper}/bin/weft-mount-helper";
    owner = "root";
    group = "users";
    setuid = true;
    permissions = "u+rx,g+x";
  };
  services.dbus.enable = true;

  services.udev.packages = [ pkgs.libinput ];

  environment.systemPackages = with pkgs; [
    mesa
    wayland-utils
    libinput
    bash
    coreutils
    curl
    htop
    pkgs.weft.weft-servo-shell
    pkgs.weft.weft-app-shell
    pkgs.weft.weft-pack
  ];

  nixpkgs.overlays = [
    rustOverlay
    (final: prev: {
      rust193 = final.rust-bin.stable."1.93.0".default;
      weft = final.callPackage ./weft-packages.nix {
        rust193 = final.rust193;
      };
    })
  ];

  # graphical-session.target refuses a manual start; the login shell starts
  # this target, which binds it, as Wayland compositors' session targets do.
  systemd.user.targets.weft-session = {
    description = "WEFT OS session";
    bindsTo = [ "graphical-session.target" ];
    wants = [ "graphical-session-pre.target" ];
    after = [ "graphical-session-pre.target" ];
  };

  systemd.user.services = {
    weft-compositor = {
      description = "WEFT OS Wayland Compositor";
      before = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      wantedBy = [ "weft-session.target" ];
      serviceConfig = {
        Type = "notify";
        ExecStart = "${pkgs.weft.weft-compositor}/bin/weft-compositor";
        # The compositor publishes WAYLAND_DISPLAY to the user manager for the
        # units after it; a restarted compositor must not inherit it, or an
        # X11 DISPLAY, or it would pick the nested backend.
        UnsetEnvironment = "WAYLAND_DISPLAY DISPLAY";
        Restart = "on-failure";
        RestartSec = "1";
      };
    };

    weft-servo-shell = {
      description = "WEFT OS System Shell";
      requires = [ "weft-compositor.service" ];
      after = [ "weft-compositor.service" ];
      partOf = [ "graphical-session.target" ];
      wantedBy = [ "weft-session.target" ];
      environment = {
        WEFT_SYSTEM_UI_HTML = "${pkgs.weft.weft-servo-shell}/share/weft/shell/system-ui.html";
      };
      serviceConfig = {
        Type = "simple";
        ExecStart = "${pkgs.weft.weft-servo-shell}/bin/weft-servo-shell";
        Restart = "on-failure";
        RestartSec = "2";
      };
    };

    weft-appd = {
      description = "WEFT Application Daemon";
      requires = [ "weft-compositor.service" ];
      after = [ "weft-compositor.service" "weft-servo-shell.service" ];
      partOf = [ "graphical-session.target" ];
      wantedBy = [ "weft-session.target" ];
      serviceConfig = {
        Type = "notify";
        ExecStart = "${pkgs.weft.weft-appd}/bin/weft-appd";
        Restart = "on-failure";
        RestartSec = "1s";
        Environment = [
          "WEFT_RUNTIME_BIN=${pkgs.weft.weft-runtime}/bin/weft-runtime"
          "WEFT_APP_SHELL_BIN=${pkgs.weft.weft-app-shell}/bin/weft-app-shell"
          "WEFT_FILE_PORTAL_BIN=${pkgs.weft.weft-file-portal}/bin/weft-file-portal"
          "WEFT_MOUNT_HELPER=/run/wrappers/bin/weft-mount-helper"
        ];
      };
    };
  };

  programs.bash.loginShellInit = ''
    if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ] && [ "$(tty)" = "/dev/tty1" ]; then
      systemctl --user start weft-session.target
    fi
  '';

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "root" "weft" ];
  };
}
