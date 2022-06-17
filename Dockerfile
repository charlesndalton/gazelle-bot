FROM docker.io/clux/muslrust:1.59.0 as cargo-build
WORKDIR /tmp
RUN cargo install cargo-build-deps
RUN USER=root cargo new --bin gazelle
WORKDIR /tmp/gazelle
COPY Cargo.toml Cargo.lock ./
RUN cargo build-deps --release
RUN rm -rf tmp/gazelle/src
COPY src tmp/gazelle/src 
RUN env CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --target x86_64-unknown-linux-musl --release


FROM docker.io/alpine:latest

RUN apk add --no-cache tini

COPY --from=cargo-build /tmp/gazelle/target/x86_64-unknown-linux-musl/release/gazelle /
WORKDIR /

ENV RUST_LOG=INFO
CMD ["./gazelle"]
