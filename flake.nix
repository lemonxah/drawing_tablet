{
  description = "Drawing Tablet - Turn your Android device into a professional graphics tablet for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # ── Overlay ────────────────────────────────────────────────────────
      # Consumers can apply this overlay to their own nixpkgs instance:
      #   nixpkgs.overlays = [ drawing-tablet.overlays.default ];
      #   environment.systemPackages = [ pkgs.drawing-tablet ];
      overlay = final: prev:
        let
          gstPlugins = with final.gst_all_1; [
            gstreamer
            gst-plugins-base
            gst-plugins-good
            gst-plugins-bad
            gst-vaapi
          ];

          runtimeLibs = with final; [
            wayland
            libxkbcommon
            libGL
            vulkan-loader
          ];
        in {
          drawing-tablet = final.rustPlatform.buildRustPackage {
            pname = "drawing-tablet";
            version = "0.1.0";

            src = final.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # --- build inputs -------------------------------------------------
            nativeBuildInputs = with final; [
              pkg-config
              clang
              rustPlatform.bindgenHook
              makeWrapper
            ];

            buildInputs = with final; [
              # PipeWire
              pipewire

              # GStreamer (dev headers)
              gst_all_1.gstreamer
              gst_all_1.gst-plugins-base

              # GLib / system
              glib

              # eframe / egui (Wayland + GPU)
              wayland
              libxkbcommon
              libGL
              vulkan-loader
              libx11
              libxcursor
              libxrandr
              libxi
            ];

            # Only build the server binary
            cargoBuildFlags = [ "--bin" "dt-server" ];
            cargoTestFlags  = [ "--bin" "dt-server" ];

            # --- install phase ------------------------------------------------
            postInstall = ''
              # Rename binary to match desktop file expectations
              mv $out/bin/dt-server $out/bin/drawing-tablet

              # Desktop file
              install -Dm644 pkg/drawing-tablet.desktop \
                $out/share/applications/drawing-tablet.desktop

              # Icon
              install -Dm644 crates/dt-server/assets/icon.png \
                $out/share/icons/hicolor/256x256/apps/drawing-tablet.png

              # License & docs
              install -Dm644 LICENSE $out/share/licenses/drawing-tablet/LICENSE
              install -Dm644 README.md $out/share/doc/drawing-tablet/README.md

              # Wrap the binary so it can find GStreamer plugins and Wayland/GL libs
              wrapProgram $out/bin/drawing-tablet \
                --prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${final.lib.makeSearchPath "lib/gstreamer-1.0" gstPlugins}" \
                --prefix LD_LIBRARY_PATH : "${final.lib.makeLibraryPath runtimeLibs}"
            '';

            meta = with final.lib; {
              description = "Stream your desktop to an Android tablet and use it as a drawing tablet";
              homepage = "https://github.com/lemonxah/drawing_tablet";
              license = licenses.mit;
              platforms = platforms.linux;
              mainProgram = "drawing-tablet";
            };
          };
        };

    in
    {
      # Overlay for consumers to apply to their own nixpkgs
      overlays.default = overlay;

    } // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };

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
      in {
        packages = {
          drawing-tablet = pkgs.drawing-tablet;
          default = pkgs.drawing-tablet;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ pkgs.drawing-tablet ];

          packages = with pkgs; [
            just
            cargo-watch
            rust-analyzer
          ];

          # Ensure GStreamer plugins and GL/Wayland libs are available during dev
          shellHook = ''
            export GST_PLUGIN_SYSTEM_PATH_1_0="${pkgs.lib.makeSearchPath "lib/gstreamer-1.0" gstPlugins}"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeLibs}:$LD_LIBRARY_PATH"
          '';
        };

        # ── Android (placeholder) ────────────────────────────────────────
        # TODO: Add android build outputs here.
        # Potential approaches:
        #   • pkgs.androidenv.composeAndroidPackages + Gradle wrapper
        #   • A passthru script that invokes `./gradlew assembleRelease`
      }
    );
}
