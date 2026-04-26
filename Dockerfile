# Build environment for launa-server (aarch64 / RPi Zero 2W)
#
# This image is a persistent build environment — like a venv.
# It contains bun, rust, and the aarch64 cross-compilation toolchain.
# The deploy script mounts source code and builds fresh each time.
#
# Usage:
#   docker build -t launa-builder .          # (re)build the environment
#   ./deploy.sh                               # build artifacts + deploy to Pi

FROM --platform=linux/amd64 rust:1-slim

# aarch64 cross-compilation toolchain
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
    ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add aarch64-unknown-linux-gnu

# Install bun
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH="/root/.bun/bin:${PATH}"

# Cross-compilation env
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CFLAGS_aarch64_unknown_linux_gnu="-I/usr/aarch64-linux-gnu/include"

WORKDIR /project
