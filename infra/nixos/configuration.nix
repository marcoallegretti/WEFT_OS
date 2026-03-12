{ pkgs, lib, self, modulesPath, ... }:

{
  imports = [
    "${modulesPath}/profiles/qemu-guest.nix"
  ];

  system.stateVersion = "24.11";

  boot.loader.grub = {
    enable = true;
    device = "/dev/vda";
  };

  fileSystems."/" = {
    device = "/dev/vda1";
    fsType = "ext4";
  };

  virtualisation = {
    qemu.options = [ "-vga virtio" "-display gtk,gl=on" ];
    memorySize = 4096;
    cores = 4;
    diskSize = 20480;
  };

  hardware.opengl = {
    enable = true;
    driSupport = true;
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
  ];

  nixpkgs.overlays = [
    (final: prev: {
      weft = final.callPackage ./weft-packages.nix { };
    })
  ];

  systemd.user.services = {
    weft-compositor = {
      description = "WEFT OS Wayland Compositor";
      after = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      wantedBy = [ "graphical-session.target" ];
      serviceConfig = {
        Type = "notify";
        ExecStart = "${pkgs.weft.weft-compositor}/bin/weft-compositor";
        Restart = "on-failure";
        RestartSec = "1";
      };
    };

    weft-appd = {
      description = "WEFT Application Daemon";
      requires = [ "weft-compositor.service" ];
      after = [ "weft-compositor.service" "weft-servo-shell.service" ];
      serviceConfig = {
        Type = "notify";
        ExecStart = "${pkgs.weft.weft-appd}/bin/weft-appd";
        Restart = "on-failure";
        RestartSec = "1s";
        Environment = [
          "WEFT_RUNTIME_BIN=${pkgs.weft.weft-runtime}/bin/weft-runtime"
          "WEFT_FILE_PORTAL_BIN=${pkgs.weft.weft-file-portal}/bin/weft-file-portal"
          "WEFT_MOUNT_HELPER=${pkgs.weft.weft-mount-helper}/bin/weft-mount-helper"
        ];
      };
    };
  };

  programs.bash.loginShellInit = ''
    if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ] && [ "$(tty)" = "/dev/tty1" ]; then
      systemctl --user start graphical-session.target
    fi
  '';

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "root" "weft" ];
  };
}
