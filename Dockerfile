# Build Stage
FROM --platform="${BUILDPLATFORM}" rust:1.92.0-slim-trixie
USER 0:0
WORKDIR /home/rust/src

ARG TARGETARCH

ARG CARGO_BUILD_JOBS=10
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

RUN case "${TARGETARCH}" in \
      amd64) linker="x86_64-linux-gnu-gcc"; linker_package="gcc-x86-64-linux-gnu"; build_target="x86_64-unknown-linux-gnu" ;; \
      arm64) linker="aarch64-linux-gnu-gcc"; linker_package="gcc-aarch64-linux-gnu"; build_target="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    dpkg --add-architecture "${TARGETARCH}" && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
      make \
      pkg-config \
      "${linker_package}" \
      "libdav1d-dev:${TARGETARCH}" \
      "libssl-dev:${TARGETARCH}" && \
    rustup target add "${build_target}"

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN case "${TARGETARCH}" in \
      amd64) linker="x86_64-linux-gnu-gcc"; build_target="x86_64-unknown-linux-gnu" ;; \
      arm64) linker="aarch64-linux-gnu-gcc"; build_target="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    export RUSTFLAGS="-C linker=${linker}" && \
    export PKG_CONFIG_ALLOW_CROSS=1 && \
    export PKG_CONFIG_PATH="/usr/lib/pkgconfig:/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig" && \
    cargo build -j "${CARGO_BUILD_JOBS}" --locked --release --target "${build_target}" && \
    mv target /tmp/cargo-target && \
    mv "/tmp/cargo-target/${build_target}" target
