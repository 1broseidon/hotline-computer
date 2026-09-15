# The same glibc userland builds and runs the computer. GUI and development
# workloads need Mesa even when the display has no hardware GPU.
FROM rust:1-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake perl pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY skills ./skills
ARG TOAD_BUILD_REVISION=unknown
ARG TOAD_BUILD_CHANNEL=development
ENV TOAD_BUILD_REVISION=$TOAD_BUILD_REVISION TOAD_BUILD_CHANNEL=$TOAD_BUILD_CHANNEL
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target cargo build --release --locked && cp target/release/toad-computer /usr/local/bin/toad-computer

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
    fonts-dejavu-core fonts-noto-color-emoji xfonts-base alacritty \
    libgl1-mesa-dri libegl-mesa0 libglx-mesa0 mesa-utils \
    libgtk-3-0t64 libwebkit2gtk-4.1-0 libayatana-appindicator3-1 librsvg2-common \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home --shell /bin/bash agent \
    && install -d /etc/nix \
    && install -d -m 1777 /tmp/.X11-unix \
    && install -d -o agent -g agent /nix /nix/store /nix/var/nix /home/agent/.config/alacritty \
    && printf 'experimental-features = nix-command flakes\nsandbox = false\nbuild-users-group =\n' > /etc/nix/nix.conf \
    && chown -R agent:agent /nix \
    && printf '[window]\ndynamic_title = false\n[font]\nsize = 12.0\n' > /home/agent/.config/alacritty/alacritty.toml \
    && chown -R agent:agent /home/agent
COPY --from=build /usr/local/bin/toad-computer /usr/bin/toad-computer
USER agent
WORKDIR /home/agent
ENV TOAD_COMPUTER_ADDR=0.0.0.0:8787 \
    TOAD_COMPUTER_HOME=/home/agent \
    TOAD_COMPUTER_SCREEN=1920x1080 \
    DISPLAY=:0 \
    ACCESSIBILITY_ENABLED=1 \
    LANG=C.UTF-8 \
    NIX_REMOTE=local \
    NIX_SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    LIBGL_ALWAYS_SOFTWARE=1 \
    GDK_BACKEND=x11
RUN install -d /home/agent/src && nix-store --init
EXPOSE 8787
ENTRYPOINT ["/usr/bin/toad-computer", "boot"]
