{
  description = "Follow video creators";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils, ... }@inputs:
    (flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
          gettext-patched = pkgs.gettext.overrideAttrs (prev: self: rec {
            version = "0.23";
            src = pkgs.fetchurl {
              url = "mirror://gnu/gettext/${self.pname}-${version}.tar.gz";
              hash = "sha256-lF3XACoC3XEIrQUQYC4TQWtB0yeJjPhSIgG8avEJB6Y=";
            };
          });

          name = "pipeline";
          legacyname = "tubefeeder";
          appid = "de.schmidhuberj.tubefeeder";
        in
        rec { 
          packages.default = 
            with pkgs;
            stdenv.mkDerivation rec {
              cargoDeps = rustPlatform.importCargoLock {
                lockFile = ./Cargo.lock;

                outputHashes = {
                  "pipeline-api-0.1.0" = "sha256-7iRIZcKmJD9+xHGAx3lAiEI+BMPjKVlFMsGBU9gcflA=";
                  "pipeline-core-0.1.0" = "sha256-7iRIZcKmJD9+xHGAx3lAiEI+BMPjKVlFMsGBU9gcflA=";
                  "pipeline-provider-piped-0.1.0" = "sha256-7iRIZcKmJD9+xHGAx3lAiEI+BMPjKVlFMsGBU9gcflA=";
                  "pipeline-provider-peertube-0.1.0" = "sha256-7iRIZcKmJD9+xHGAx3lAiEI+BMPjKVlFMsGBU9gcflA=";
                  "piped-openapi-sdk-1.0.0" = "sha256-7iRIZcKmJD9+xHGAx3lAiEI+BMPjKVlFMsGBU9gcflA=";
                  "rustypipe-0.11.4" = "sha256-PFI2/cDAO/ahIf6rt/JDvu5INbHBK7PWMTYhjrvlObc=";
                };
              };
              src = let fs = lib.fileset; in fs.toSource {
                root = ./.;
                fileset =
                  fs.difference
                    ./.
                    (fs.unions [
                      (fs.maybeMissing ./result)
                      (fs.maybeMissing ./build)
                      ./flake.nix
                      ./flake.lock
                    ]);
              };

              buildInputs = with pkgs; [ pkgs.libadwaita pkgs.clapper-unwrapped pkgs.clapper-enhancers pkgs.yt-dlp pkgs.gst_all_1.gstreamer pkgs.gst_all_1.gst-plugins-base pkgs.gst_all_1.gst-plugins-good pkgs.gst_all_1.gst-plugins-bad pkgs.gst_all_1.gst-plugins-ugly pkgs.gst_all_1.gst-libav pkgs.gst_all_1.gst-vaapi pkgs.glib-networking sqlite ];
              nativeBuildInputs = with pkgs; [ pkgs.clapper-unwrapped pkgs.clapper-enhancers pkgs.yt-dlp pkgs.wrapGAppsHook4 rustPlatform.cargoSetupHook meson gettext pkgs.glib pkg-config desktop-file-utils appstream ninja rustc cargo openssl pkgs.blueprint-compiler pkgs.glib-networking gettext-patched ];

              inherit name;
            };
          devShells.default =
            let 
              run = pkgs.writeShellScriptBin "run" ''
                meson compile -C build && ./build/target/debug/${legacyname}
              '';
              debug = pkgs.writeShellScriptBin "debug" ''
                meson compile -C build && gdb ./build/target/debug/${legacyname}
              '';
              check = pkgs.writeShellScriptBin "check" ''
                cargo clippy
              '';
              format = pkgs.writeShellScriptBin "format" ''
                cargo fmt
                python3 -m json.tool build-aux/${appid}.json build-aux/${appid}.json
                xmllint --format --recover data/resources/resources.gresource.xml -o data/resources/resources.gresource.xml
                xmllint --format --recover data/${appid}.gschema.xml -o data/${appid}.gschema.xml
                xmllint --format --recover data/${appid}.metainfo.xml -o data/${appid}.metainfo.xml
              '';
              prof = pkgs.writeShellScriptBin "prof" ''
                meson compile -C build
                sysprof-cli --force --no-battery --use-trace-fd --speedtrack --gtk $@ tubefeeder.syscap -- ./build/target/debug/${legacyname}
              '';
            in
            with pkgs;
            pkgs.mkShell {
              src = ./.;
              buildInputs = self.packages.${system}.default.buildInputs ++ [ pkgs.glib-networking ];
              nativeBuildInputs = self.packages.${system}.default.nativeBuildInputs ++ [ rustfmt python3 libxml2 gdb cargo-deny clippy pkgs.glib-networking sysprof pkgs.md4c ] ++ [ run check format debug prof ];
              shellHook = ''
                # export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:${pkgs.glib-networking}/lib/gio/modules
                export PATH=${gettext-patched}/bin:$PATH
                meson setup -Dprofile=development build
              '';
              GSETTINGS_SCHEMA_DIR = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}/glib-2.0/schemas/:${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}/glib-2.0/schemas/:./build/data/";
              GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules/";


              # Source: https://github.com/NixOS/nixpkgs/blob/ca49c4304acf0973078db0a9d200fd2bae75676d/pkgs/by-name/cl/clapper/package.nix#L38C44-L38C107
              CLAPPER_ENHANCERS_PATH = "${clapper-enhancers}/${clapper-enhancers.passthru.pluginPath}";

              GDK_PIXBUF_MODULE_FILE="${pkgs.gnome._gdkPixbufCacheBuilder_DO_NOT_USE {
                                extraLoaders = [ 
                                  pkgs.libjxl 
                                  pkgs.librsvg 
                                  pkgs.webp-pixbuf-loader 
                                  pkgs.libheif.lib
                                ]; 
                              }}";

              RUSTFLAGS="-C force-frame-pointers=yes";
            };
          apps.default = {
            type = "app";
            inherit name;
            program = "${self.packages.${system}.default}/bin/${name}";
          };

          # Note: This may only be run interactively as this requires network access.
          # It requires manually picking a channel to display, waiting one minute for the screenshot to be taken, selecting a video to play and waiting another minute.
          # It furthermore requires a `screenshot-aux` directory containing the `data.sqlite` and `~/.cache/tubefeeder` (renamed to `cache`) which will be copied to the VM.
          packages.makeScreenshot =
            let
              nixos-lib = import (nixpkgs + "/nixos/lib") { };
            in
            nixos-lib.runTest {
              name = "screenshot";
              hostPkgs = pkgs;
              imports = [
                {
                  nodes = {
                    machine = { pkgs, modulesPath, ... }: {
                      boot.loader.systemd-boot.enable = true;
                      boot.loader.efi.canTouchEfiVariables = true;

                      services.xserver.enable = true;
                      services.displayManager.gdm.enable = true;
                      services.desktopManager.gnome.enable = true;
                      services.displayManager.autoLogin.enable = true;
                      services.displayManager.autoLogin.user = "alice";

                      users.users.alice = {
                        isNormalUser = true;
                        extraGroups = [ "wheel" ];
                        uid = 1000;
                      };

                      system.stateVersion = "22.05";

                      virtualisation.qemu.options = [ "-device VGA,edid=on,xres=1400,yres=750" ]; # Source: https://wiki.archlinux.org/title/QEMU
                      virtualisation.memorySize = 4096;

                      environment.systemPackages = [
                        self.packages.${system}.default
                        pkgs.clapper-unwrapped pkgs.clapper-enhancers pkgs.yt-dlp
                      ];

                      environment.sessionVariables = {
                        RUST_LOG="tubefeeder=trace";
                        CLAPPER_ENHANCERS_PATH = "${pkgs.clapper-enhancers}/${pkgs.clapper-enhancers.passthru.pluginPath}";
                      };

                      systemd.user.services = {
                        "org.gnome.Shell@wayland" = {
                          serviceConfig = {
                            ExecStart = [
                              ""
                              "${pkgs.gnome-shell}/bin/gnome-shell"
                            ];
                          };
                        };
                      };
                    };
                  };

                  testScript = { nodes, ... }:
                    let
                      lib = pkgs.lib;
                      l = lib.lists;

                      user = nodes.machine.users.users.alice;
                      username = user.name;

                      type = word: "machine.send_chars(\"${word}\")";
                      key = key: "machine.send_key(\"${key}\")";
                      sleep = duration: "machine.sleep(${toString duration})";

                      execution = [
                        (key "tab")
                        (key "tab")
                        (key "tab")
                        (key "tab")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "ret")

                        (sleep 5)
                        (key "alt-print") # XXX: This for some reason sometimes fails. No idea why.
                        "machine.execute(\"mv /home/${username}/Pictures/Screenshots/* screenshot-subscriptions.png\")"
                        "machine.copy_from_vm(\"screenshot-subscriptions.png\", \".\")"

                        (key "tab")
                        (key "tab")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "ret")

                        (sleep 5)
                        (key "alt-print") # XXX: This for some reason sometimes fails. No idea why.
                        "machine.execute(\"mv /home/${username}/Pictures/Screenshots/* screenshot-channel.png\")"
                        "machine.copy_from_vm(\"screenshot-channel.png\", \".\")"

                        (key "tab")
                        (key "shift-tab")
                        (key "shift-tab")
                        (key "shift-tab")
                        (key "shift-tab")
                        (key "up")
                        (key "up")
                        (key "up")
                        (key "ret")
                        (key "tab")
                        (key "down")
                        (key "down")
                        (key "down")
                        (key "ret")
                        (sleep 180) # XXX: Staring a video inside the VM takes ages for some reason. Maybe due to missing hardware acceleration.
                        (key "spc")
                        (key "0")
                        (sleep 5)
                        (key "right")
                        (key "right")
                        (key "right")

                        (sleep 5)
                        (key "alt-print") # XXX: This for some reason sometimes fails. No idea why.
                        "machine.execute(\"mv /home/${username}/Pictures/Screenshots/* screenshot-video.png\")"
                        "machine.copy_from_vm(\"screenshot-video.png\", \".\")"

                        (key "esc")

                        (sleep 10)
                        (key "alt-print") # XXX: This for some reason sometimes fails. No idea why.
                        "machine.execute(\"mv /home/${username}/Pictures/Screenshots/* screenshot-watch-later.png\")"
                        "machine.copy_from_vm(\"screenshot-watch-later.png\", \".\")"
                      ];


                      preExecution = [
                        (sleep 20)
                        "machine.copy_from_host(\"./screenshot-aux/data.sqlite\", \"/home/alice/.local/share/tubefeeder/data.sqlite\")"
                        "machine.copy_from_host(\"./screenshot-aux/data.sqlite-wal\", \"/home/alice/.local/share/tubefeeder/data.sqlite-wal\")"
                        "machine.copy_from_host(\"./screenshot-aux/data.sqlite-shm\", \"/home/alice/.local/share/tubefeeder/data.sqlite-shm\")"
                        "machine.copy_from_host(\"./screenshot-aux/cache\", \"/home/alice/.cache/tubefeeder\")"
                        (sleep 20)
                        (type "Pipeline")
                        (sleep 1)
                        (key "ret")
                      ];

                      fullExecution = l.flatten [preExecution (sleep 20) execution];

                      code = lib.concatStringsSep "\nmachine.sleep(1)\n" fullExecution;
                    in
                      code;
                }
              ];
            };
        })
    );
}
