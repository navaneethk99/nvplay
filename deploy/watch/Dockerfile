FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/src/target --mount=type=cache,target=/usr/local/cargo/registry \
    CARGO_BUILD_JOBS=2 cargo build --locked --release -p nvplay-watch-server && \
    cp target/release/nvplay-watch-server /usr/local/bin/nvplay-watch-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 watch
COPY --from=build /usr/local/bin/nvplay-watch-server /usr/local/bin/nvplay-watch-server
USER watch
ENV NVPLAY_WATCH_BIND=0.0.0.0:8787
EXPOSE 8787
ENTRYPOINT ["nvplay-watch-server"]
