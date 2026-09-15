let
 pkgs = import (builtins.fetchTarball "https://codeload.github.com/NixOS/nixpkgs/tar.gz/ef34387ddd751e1ab8857adf4676492d32eb24ec") {};
in pkgs.mkShell {
 nativeBuildInputs = with pkgs; [ cargo rustc bun nodejs pkg-config gitMinimal cmake perl xterm ];
 buildInputs = with pkgs; [ gtk3 webkitgtk_4_1 libsoup_3 openssl libayatana-appindicator librsvg ];
 shellHook = ''
  export GDK_BACKEND=x11
  export TOAD_DATA_DIR=/home/agent/qa/toad-data
  export CARGO_BUILD_JOBS=2
  export CARGO_PROFILE_DEV_DEBUG=0
 '';
}
