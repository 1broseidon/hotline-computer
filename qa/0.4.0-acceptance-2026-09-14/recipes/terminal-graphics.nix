let pkgs = import /nix/store/hhfnpma32czw4h3bqpag8dciax0bcmar-source {}; in pkgs.mkShell {
 packages = with pkgs; [ alacritty ghostty mesa.drivers libglvnd ];
 shellHook = ''
  export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [ pkgs.libglvnd ]}
  export LIBGL_DRIVERS_PATH=${pkgs.mesa.drivers}/lib/dri
  export __EGL_VENDOR_LIBRARY_FILENAMES=${pkgs.mesa.drivers}/share/glvnd/egl_vendor.d/50_mesa.json
  export LIBGL_ALWAYS_SOFTWARE=1
 '';
}
