# The same glibc userland builds and runs the computer. GUI and development
# workloads need Mesa even when the display has no hardware GPU.
FROM rust:1-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake perl pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY skills ./skills
ARG HOTLINE_BUILD_REVISION=unknown
ARG HOTLINE_BUILD_CHANNEL=development
ENV HOTLINE_BUILD_REVISION=$HOTLINE_BUILD_REVISION HOTLINE_BUILD_CHANNEL=$HOTLINE_BUILD_CHANNEL
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target cargo build --release --locked && cp target/release/hotline-computer /usr/local/bin/hotline-computer

FROM build AS checks
RUN rustup component add rustfmt clippy
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target rm -f target/debug/deps/contract-* && cargo fmt --check && CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --locked && CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo clippy --all-targets --locked -- -D warnings \
    && mkdir -p /acceptance \
    && for binary in target/debug/deps/contract-*; do if [ -f "$binary" ] && [ -x "$binary" ]; then cp "$binary" /acceptance/contract; fi; done \
    && test -x /acceptance/contract

FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    bash git curl wget ca-certificates tar gzip bzip2 xz-utils unzip zip ripgrep jq file \
    nix-bin python3 xvfb xauth x11-xkb-utils dbus at-spi2-core chromium \
    fonts-dejavu-core fonts-noto-color-emoji xfonts-base alacritty tzdata \
    libgl1-mesa-dri libegl-mesa0 libglx-mesa0 mesa-utils \
    libgtk-3-0t64 librsvg2-common \
    # The Secret Service native apps keep passwords in, unlocked from boot,
    # and the tool that reads it from a shell.
    gnome-keyring libsecret-tools \
    # What AppImage tooling never bundles because every desktop is assumed
    # to have it (the AppImage exclude list), beyond glibc, Mesa and GTK above.
    libgpg-error0 libopengl0 libxcb-dri2-0 libjack-jackd2-0 libpipewire-0.3-0 libusb-1.0-0 \
    && rm -rf /var/lib/apt/lists/* \
    # Rootless by design: nothing runs as root, so apt could never install
    # anything. It goes, so nobody is invited to try; software comes from
    # hotline-computer, then Nix, then what the image already has.
    && rm -f /usr/bin/apt /usr/bin/apt-* \
    # Boot starts the keyring unlocked. Without these, a client that asks
    # for secrets while it is being restarted would have the bus start a
    # locked one in its place.
    && rm -f /usr/share/dbus-1/services/org.freedesktop.secrets.service \
             /usr/share/dbus-1/services/org.gnome.keyring.service \
             /usr/share/dbus-1/services/org.freedesktop.impl.portal.Secret.service \
    && useradd --uid 1000 --create-home --shell /bin/bash agent \
    && rm -f /home/agent/.bashrc /home/agent/.profile /home/agent/.bash_logout \
    && install -d /etc/nix \
    && install -d -m 1777 /tmp/.X11-unix \
    && install -d -o agent -g agent /nix /nix/store /nix/var/nix \
    && printf 'experimental-features = nix-command flakes\nsandbox = false\nbuild-users-group =\n!include /opt/hotline-computer/nix.conf\n' > /etc/nix/nix.conf \
    && install -d -o agent -g agent /opt/hotline-computer \
    && chown -R agent:agent /nix \
    && chown -R agent:agent /home/agent
COPY assets/alacritty.toml /etc/hotline-computer/alacritty.toml
# The base every workspace manifest composes over: the Nixpkgs pin and the
# platforms and services this image provides. An image release is a new one.
COPY assets/base.json /etc/hotline-computer/base.json
COPY assets/chromium-policy.json /etc/chromium/policies/managed/hotline.json
COPY assets/bashrc /etc/bash.bashrc
# "Show in folder" in the browser, and xdg-open on a folder, open the
# person's terminal there.
COPY assets/hotline-open-folder.desktop assets/mimeapps.list /usr/share/applications/
COPY --from=build /usr/local/bin/hotline-computer /usr/bin/hotline-computer
USER agent
WORKDIR /home/agent
ENV HOTLINE_COMPUTER_ADDR=0.0.0.0:8787 \
    HOTLINE_COMPUTER_HOME=/home/agent \
    HOTLINE_COMPUTER_SCREEN=1920x1080 \
    DISPLAY=:0 \
    ACCESSIBILITY_ENABLED=1 \
    LANG=C.UTF-8 \
    NIX_REMOTE=local \
    NIX_SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    LIBGL_ALWAYS_SOFTWARE=1 \
    GDK_BACKEND=x11 \
    APPIMAGE_EXTRACT_AND_RUN=1
RUN install -d /home/agent/src && nix-store --init
# The base's closure ships as a signed binary cache outside /nix, not as
# store contents: the desk mounts /nix as a shared volume that already holds
# older images' paths, so only a substituter reaches every computer. Nix
# takes from it before the network. HOTLINE_PREREALISE names platforms to
# include too (for example "webkit"), at the cost of image size.
ARG HOTLINE_PREREALISE=""
RUN rev=$(jq -r .nixpkgs /etc/hotline-computer/base.json) \
    && cache=/opt/hotline-computer/base-cache \
    && targets="github:NixOS/nixpkgs/$rev#stdenv github:NixOS/nixpkgs/$rev#bashInteractive" \
    && for platform in $HOTLINE_PREREALISE; do \
         targets="$targets $(jq -r --arg p "$platform" '.platforms as $all | def need($n): ($all[$n].requires // [] | map(need(.)) | add // []) + [$n]; [need($p)[] | $all[.] | (.packages + .libraries)[]] | unique | .[]' /etc/hotline-computer/base.json | sed "s|^|github:NixOS/nixpkgs/$rev#|" | tr '\n' ' ')"; \
       done \
    && nix build --no-link --print-out-paths $targets > /tmp/base-paths \
    && nix key generate-secret --key-name hotline-computer-base > /tmp/base.key \
    && nix store sign --key-file /tmp/base.key --recursive $(cat /tmp/base-paths) \
    && nix copy --to "file://$cache?compression=zstd" $(cat /tmp/base-paths) \
    && nix flake archive --to "file://$cache?compression=zstd" "github:NixOS/nixpkgs/$rev" \
    && printf 'extra-substituters = file://%s?priority=10\nextra-trusted-public-keys = %s\n' "$cache" "$(nix key convert-secret-to-public < /tmp/base.key)" > /opt/hotline-computer/nix.conf \
    && rm -f /tmp/base.key /tmp/base-paths \
    && nix-collect-garbage -d >/dev/null
EXPOSE 8787
ENTRYPOINT ["/usr/bin/hotline-computer", "boot"]
