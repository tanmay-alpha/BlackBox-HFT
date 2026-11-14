# BlackBox HFT: the complete simulator, API and web console in one image.
#
#   docker build -t blackbox-hft .
#   docker run --rm -p 8787:8787 -v blackbox-data:/data blackbox-hft
#
# The default command generates the synthetic demo dataset on first start (if the data volume has
# no dataset yet) and serves the console on port 8787. To ingest Tardis sample files you have
# downloaded yourself, mount them read-only and run the CLI, e.g.
#   docker run --rm -v blackbox-data:/data -v "$PWD/data/raw:/raw:ro" blackbox-hft \
#     blackbox --data-dir /data ingest-tardis --trades /raw/.../trades.csv.gz --book ... --id ...

FROM rust:1.94-bookworm AS rust-build
WORKDIR /src
# Rust >= 1.91.1 is required by the pinned engine.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The research report embeds docs/LIMITATIONS.md at compile time.
COPY docs docs
RUN cargo build --release --locked --bin blackbox

FROM node:22-bookworm-slim AS web-build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data blackbox \
    && mkdir -p /data /app/web \
    && chown blackbox /data
COPY --from=rust-build /src/target/release/blackbox /usr/local/bin/blackbox
COPY --from=web-build /web/dist /app/web
USER blackbox
VOLUME ["/data"]
EXPOSE 8787
# Inside the container the server must listen on all interfaces so the published port reaches
# it. Publish it to the host's loopback only (-p 127.0.0.1:8787:8787) unless you intend to share
# it: the API has no authentication.
CMD ["blackbox", "--data-dir", "/data", "demo", "--addr", "0.0.0.0:8787", "--web-dir", "/app/web"]
