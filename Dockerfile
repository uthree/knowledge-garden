# Knowledge Garden: one static-ish binary plus a SQLite database under /data.

FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY migrations migrations
COPY assets assets
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 1000 --home-dir /data garden \
    && mkdir -p /data \
    && chown garden /data
COPY --from=build /src/target/release/knowledge-garden /usr/local/bin/knowledge-garden
ENV KG_DATABASE=sqlite:///data/garden.db \
    KG_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
USER garden
ENTRYPOINT ["knowledge-garden"]
CMD ["serve"]
