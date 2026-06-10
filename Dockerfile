# Stage 1: Build using a dedicated cross-compilation image
FROM messense/rust-musl-cross:aarch64-musl AS builder

WORKDIR /usr/src/ros-igmp-querier
COPY . .

# Cross-compile to aarch64 (arm64) using native x86_64 tools
RUN cargo build --release --target=aarch64-unknown-linux-musl

# Stage 2: Minimal alpine image (Allows debugging via shell if needed)
FROM alpine:latest

# Copy the static binary
COPY --from=builder /usr/src/ros-igmp-querier/target/aarch64-unknown-linux-musl/release/ros-igmp-querier /usr/local/bin/

ENV INTERFACE=eth0
ENV VLANS=""
ENV QUERIER_IP="dynamic"
ENV INTERVAL="125"

ENTRYPOINT ["ros-igmp-querier"]
