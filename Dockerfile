# BlackBox HFT container images. Two final targets share one Rust build:
#
#   hosted  The API image deployed to Render (docs/engineering/DEPLOYMENT.md). No web assets: the
#           console is served by Vercel. Built and pushed by .github/workflows/publish.yml:
#             docker build --target hosted -t blackbox-hft:hosted .
#
#   local   (default, last stage) The complete simulator, `/api/v1` with its worker and the web
#           console in one image, for your own machine, next to a Postgres. Run it with
#           docker-compose.yml (README "Quick start"), which CI's `docker` job also smoke-tests:
#             docker compose up --build
#           The default command (scripts/docker/local-start.sh) migrates, seeds the demo workspace
#           (runs, curated comparisons, saved reports), mints and prints one research API key on
#           the first start, and serves the console on port 8787. To ingest Tardis sample files
#           you downloaded yourself, mount them read-only and run the CLI, e.g.
#             docker compose run --rm -v "$PWD/data/raw:/raw:ro" app \
#               blackbox --data-dir /data ingest-tardis --trades /raw/.../trades.csv.gz --book ... --id ...
#
# Base images are pinned to the multi-arch index digest of an exact tag. The digests were read on
# 2026-11-14 from the registry (HEAD https://registry-1.docker.io/v2/library/<repo>/manifests/<tag>,
# header Docker-Content-Digest) and match Docker Hub's tag listing. To update: pick the new tag,
# read its digest the same way, and change tag and digest together. Render runs linux/amd64 only;
# the Rust stage builds for x86_64-unknown-linux-gnu explicitly, so a build on another
# architecture fails instead of producing an image Render cannot run.
#
# Rust 1.100.0 >= the workspace rust-version (MSRV 1.94 once sqlx 0.9 lands).
ARG RUST_IMAGE=rust:1.100.0-trixie@sha256:e19840879045c951c905d0eaa9887f3439c62000e429963371b376bac8d38c31
ARG NODE_IMAGE=node:22.23.5-trixie-slim@sha256:68c76cfad89921b927eae28ba21407475e29d0ce450afcdbe70c1532db042b64
ARG RUNTIME_IMAGE=debian:trixie-20261104-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

# ---------------------------------------------------------------------------------------------
# Rust build with a dependency layer (cargo-chef): `recipe.json` changes only when a manifest or
# Cargo.lock changes, so the `cook` layer (all third-party crates) is reused from the layer cache
# for source-only commits. The CI cache (buildx `type=gha`) stores these layers between runs.
FROM ${RUST_IMAGE} AS chef
ARG CARGO_CHEF_VERSION=0.1.78
ENV CARGO_TERM_COLOR=never
RUN cargo install --locked cargo-chef --version "${CARGO_CHEF_VERSION}" \
    && rm -rf "${CARGO_HOME}/registry"
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS rust-build
ARG RUST_TARGET=x86_64-unknown-linux-gnu
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --target "${RUST_TARGET}" --bin blackbox --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The research report embeds docs/LIMITATIONS.md at compile time (crates/bb-app/src/report.rs).
# Only that file is copied, so documentation edits do not invalidate the build.
COPY docs/LIMITATIONS.md docs/LIMITATIONS.md
RUN cargo build --release --locked --target "${RUST_TARGET}" --bin blackbox \
    && cp "target/${RUST_TARGET}/release/blackbox" /blackbox

# ---------------------------------------------------------------------------------------------
# Web console (local target only).
FROM ${NODE_IMAGE} AS web-build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

# ---------------------------------------------------------------------------------------------
# Common runtime: tini as PID 1 (forwards SIGTERM/SIGINT to the binary and reaps zombies, so the
# server's graceful shutdown runs when Render stops the instance), CA roots for TLS to Neon and
# Vercel Blob, and an unprivileged user. apt package versions are not pinned: the base image is
# pinned by digest, and the two packages should take Debian security updates when rebuilt.
FROM ${RUNTIME_IMAGE} AS runtime
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends tini ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 blackbox \
    && useradd --system --uid 10001 --gid 10001 --home-dir /var/lib/blackbox --shell /usr/sbin/nologin blackbox \
    && mkdir -p /var/lib/blackbox \
    && chown 10001:10001 /var/lib/blackbox
COPY --from=rust-build /blackbox /usr/local/bin/blackbox
ENTRYPOINT ["/usr/bin/tini", "--"]

# ---------------------------------------------------------------------------------------------
# Hosted API image (Render). Process environment contract: docs/engineering/DEPLOYMENT.md §5.
# `serve` binds 0.0.0.0:$PORT when PORT is set (Render sets it; render.yaml pins 10000) and
# 127.0.0.1:8787 otherwise. The working directory is writable scratch space (synthetic datasets
# and the verified-bundle cache); Render's filesystem is ephemeral, so nothing here is durable.
FROM runtime AS hosted
ARG GIT_SHA=unknown
ENV BB_GIT_SHA=${GIT_SHA} \
    PORT=8787
WORKDIR /var/lib/blackbox
USER 10001:10001
EXPOSE 8787
CMD ["blackbox", "--data-dir", "/var/lib/blackbox", "serve"]

# ---------------------------------------------------------------------------------------------
# Local all-in-one image (default target). Inside the container the server must listen on all
# interfaces so the published port reaches it. Publish it to the host's loopback only
# (127.0.0.1:8787:8787, as docker-compose.yml does) unless you intend to share it. Needs
# DATABASE_URL (a Postgres; docker-compose.yml provides one).
FROM runtime AS local
COPY --from=web-build /web/dist /app/web
COPY scripts/docker/local-start.sh /usr/local/bin/blackbox-local-start
RUN mkdir -p /data && chown 10001:10001 /data
WORKDIR /data
USER 10001:10001
VOLUME ["/data"]
EXPOSE 8787
CMD ["/bin/sh", "/usr/local/bin/blackbox-local-start"]
