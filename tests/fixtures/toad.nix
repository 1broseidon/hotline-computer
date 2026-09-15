# Dependencies of the pinned Toad test repository, not a Computer preset.
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/@NIXPKGS@";
  outputs = { self, nixpkgs }: {
    devShells = nixpkgs.lib.genAttrs [ "aarch64-linux" "x86_64-linux" ] (system:
      let pkgs = import nixpkgs { inherit system; };
      in { default = pkgs.mkShell {
        packages = with pkgs; [ rustc cargo clang cmake perl pkg-config gnumake cargo-tauri nodejs bun openssl gtk3 webkitgtk_4_1 libsoup_3 libayatana-appindicator librsvg glib-networking gsettings-desktop-schemas mesa libglvnd ];
        hardeningDisable = [ "fortify" "fortify3" ];
        NIX_ENFORCE_PURITY = "0";
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [ gtk3 webkitgtk_4_1 libsoup_3 libayatana-appindicator librsvg mesa libglvnd ]);
        GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules";
        XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share:${pkgs.gtk3}/share:/usr/share";
      }; });
  };
}
