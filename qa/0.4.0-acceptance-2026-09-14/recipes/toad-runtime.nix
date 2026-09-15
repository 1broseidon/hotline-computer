let pkgs = import /nix/store/hhfnpma32czw4h3bqpag8dciax0bcmar-source {}; in pkgs.mkShell {
 packages = with pkgs; [ libayatana-appindicator mesa libglvnd ];
 shellHook = ''
  export GDK_BACKEND=x11
  export TOAD_DATA_DIR=/home/agent/qa/toad-data
  export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [ pkgs.libayatana-appindicator pkgs.libglvnd ]}
  export LIBGL_DRIVERS_PATH=${pkgs.mesa}/lib/dri
  export __EGL_VENDOR_LIBRARY_FILENAMES=${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json
  export LIBGL_ALWAYS_SOFTWARE=1
 '';
}
