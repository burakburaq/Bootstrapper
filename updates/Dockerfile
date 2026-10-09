FROM rust:1.97.1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services
COPY tools ./tools
RUN cargo build --locked --release -p update-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/update-server /usr/local/bin/update-server
COPY deploy/entrypoint.sh /usr/local/bin/update-entrypoint
RUN chmod 755 /usr/local/bin/update-entrypoint
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/update-entrypoint"]
CMD ["serve"]
