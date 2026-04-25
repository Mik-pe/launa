# Multi-stage build for launa-server targeting aarch64 (RPi Zero 2W)

# --- Stage 1: Build the Vue frontend ---
FROM --platform=$BUILDPLATFORM node:22-slim AS web-builder
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm install --frozen-lockfile || npm install
COPY web/ ./
RUN npm run build

# --- Stage 2: Cross-compile launa-server for aarch64 ---
FROM --platform=linux/amd64 rust:1-slim AS server-builder

# Install aarch64 cross-compilation toolchain
RUN apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross && rm -rf /var/lib/apt/lists/*
RUN rustup target add aarch64-unknown-linux-gnu

WORKDIR /project

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/launa-protocol/Cargo.toml crates/launa-protocol/Cargo.toml
COPY crates/launa-hal/Cargo.toml crates/launa-hal/Cargo.toml
COPY crates/launa-mqtt/Cargo.toml crates/launa-mqtt/Cargo.toml
COPY crates/launa-ota/Cargo.toml crates/launa-ota/Cargo.toml
COPY crates/launa-esp-ota/Cargo.toml crates/launa-esp-ota/Cargo.toml
COPY crates/launa-core/Cargo.toml crates/launa-core/Cargo.toml
COPY crates/launa-sim/Cargo.toml crates/launa-sim/Cargo.toml
COPY crates/launa-integration-tests/Cargo.toml crates/launa-integration-tests/Cargo.toml
COPY crates/launa-server/Cargo.toml crates/launa-server/Cargo.toml

# Create dummy source files so cargo can resolve the workspace graph
RUN mkdir -p crates/launa-protocol/src && touch crates/launa-protocol/src/lib.rs \
 && mkdir -p crates/launa-hal/src && touch crates/launa-hal/src/lib.rs \
 && mkdir -p crates/launa-mqtt/src && touch crates/launa-mqtt/src/lib.rs \
 && mkdir -p crates/launa-ota/src && touch crates/launa-ota/src/lib.rs \
 && mkdir -p crates/launa-esp-ota/src && touch crates/launa-esp-ota/src/lib.rs \
 && mkdir -p crates/launa-core/src && touch crates/launa-core/src/lib.rs \
 && mkdir -p crates/launa-sim/src && touch crates/launa-sim/src/lib.rs \
 && mkdir -p crates/launa-integration-tests/src && touch crates/launa-integration-tests/src/lib.rs \
 && mkdir -p crates/launa-server/src/bin \
 && echo "fn main(){}" > crates/launa-server/src/bin/launa-server.rs \
 && echo "" > crates/launa-server/src/lib.rs

# Build dependencies only (cached layer). Expected to fail on real code.
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CFLAGS_aarch64_unknown_linux_gnu="-I/usr/aarch64-linux-gnu/include"
RUN cargo build --release --target aarch64-unknown-linux-gnu -p launa-server 2>/dev/null || true

# Copy real source code (overwrites dummy files)
COPY crates/ crates/

# Build the real binary
RUN touch crates/*/src/*.rs && cargo build --release --target aarch64-unknown-linux-gnu -p launa-server

# --- Stage 3: Minimal runtime image ---
FROM --platform=linux/arm64 debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=server-builder /project/target/aarch64-unknown-linux-gnu/release/launa-server /usr/local/bin/launa-server
COPY --from=web-builder /build/web/dist /var/lib/launa/web/dist

EXPOSE 1883 9001 80

VOLUME ["/var/lib/launa"]

ENTRYPOINT ["launa-server"]
CMD ["--web-dir", "/var/lib/launa/web", "--db-path", "/var/lib/launa/launa.db"]
