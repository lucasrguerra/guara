# syntax=docker/dockerfile:1
# Guará CLI image. Intended for Linux hosts, headless servers and forensic image files.
# The graphical interface is not served from the container (see docs/docker.md).

FROM rust:1-slim-bookworm AS build
WORKDIR /src
# The CLI embeds the web interface at compile time (include_str!), so it needs apps/gui/ui.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY apps/gui/ui apps/gui/ui
RUN cargo build --release --locked -p guara-cli

FROM debian:bookworm-slim
# smartmontools gives `guara health` access to SMART data.
RUN apt-get update \
 && apt-get install -y --no-install-recommends smartmontools ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/guara /usr/local/bin/guara
LABEL org.opencontainers.image.title="Guará" \
      org.opencontainers.image.description="File recovery for hard drives, SSDs and USB sticks (CLI)" \
      org.opencontainers.image.source="https://github.com/lucasrguerra/guara" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
WORKDIR /data
ENTRYPOINT ["guara"]
CMD ["--help"]
