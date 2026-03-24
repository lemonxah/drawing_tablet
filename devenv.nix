{ pkgs, lib, ... }:

let
  gstPlugins = with pkgs.gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-vaapi
  ];

  runtimeLibs = with pkgs; [
    wayland
    libxkbcommon
    libGL
    vulkan-loader
  ];
in
{
  # ── Rust ──────────────────────────────────────────────────────────────
  languages.rust = {
    enable = true;
    channel = "stable";
  };

  # ── Native dependencies ──────────────────────────────────────────────
  packages = with pkgs; [
    # Build tooling
    pkg-config
    clang
    just
    cargo-watch

    # PipeWire
    pipewire

    # GStreamer (headers + runtime plugins)
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-vaapi

    # GLib
    glib

    # Wayland / GPU (for eframe / egui)
    wayland
    libxkbcommon
    libGL
    vulkan-loader
    libx11
    libxcursor
    libxrandr
    libxi
  ];

  # ── Environment variables ────────────────────────────────────────────
  env = {
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    GST_PLUGIN_SYSTEM_PATH_1_0 = lib.makeSearchPath "lib/gstreamer-1.0" gstPlugins;
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
  };

  # ── Shell greeting ───────────────────────────────────────────────────
  enterShell = ''
    echo ""
    echo "  🖊️  Drawing Tablet dev environment"
    echo "  ──────────────────────────────────"
    echo "  just build          → debug build"
    echo "  just run-release    → run server"
    echo "  just android-build  → build APK"
    echo "  just --list         → all commands"
    echo ""
  '';

  # ── Android (future) ─────────────────────────────────────────────────
  # Uncomment when ready to develop the Android app inside devenv:
  #
  # languages.java = {
  #   enable = true;
  #   jdk.package = pkgs.jdk17;
  # };
  #
  # android = {
  #   enable = true;
  #   platforms.version = [ "34" ];
  #   buildTools.version = [ "34.0.0" ];
  #   ndk.enable = true;
  # };
}
