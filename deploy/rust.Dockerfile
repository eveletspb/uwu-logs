FROM rust:1-slim-trixie AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY config/encounters.json config/combat-rules.json config/
COPY src/ src/
RUN cargo build --release --locked --jobs 8

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends 7zip curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --no-create-home uwu \
    && mkdir -p /data/LogsDir /data/uploads \
    && chown -R uwu /data
COPY --from=build /build/target/release/uwu-logs /usr/local/bin/uwu-logs
COPY templates/ /app/templates/
COPY static/ /app/static/
ENV UWU_ROOT=/data UWU_ASSETS=/app UWU_BIND=0.0.0.0:5000 UWU_7Z=7z
WORKDIR /app
USER uwu
CMD ["uwu-logs"]
