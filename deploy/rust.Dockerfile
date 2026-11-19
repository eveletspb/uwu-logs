FROM rust:1-slim-trixie AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY config/encounters.json config/combat-rules.json config/
COPY src/ src/
COPY static/aura_icons.json static/
RUN cargo build --release --locked --jobs 8

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends 7zip curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --no-create-home raidledger \
    && mkdir -p /data/LogsDir /data/uploads \
    && chown -R raidledger /data
COPY --from=build /build/target/release/raidledger /usr/local/bin/raidledger
COPY templates/ /app/templates/
COPY static/ /app/static/
RUN chmod -R a+rX /app/templates /app/static
ENV RAIDLEDGER_ROOT=/data RAIDLEDGER_ASSETS=/app RAIDLEDGER_BIND=0.0.0.0:5000 RAIDLEDGER_7Z=7z
WORKDIR /app
USER raidledger
CMD ["raidledger"]
