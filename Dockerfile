FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM alpine:3.23
RUN apk add --no-cache ca-certificates \
    && addgroup -S app && adduser -S app -G app
LABEL org.opencontainers.image.source="https://github.com/itwormz/remna-sub-injector"
WORKDIR /app
COPY --from=builder /build/target/release/sub-injector ./sub-injector
USER app
EXPOSE 3020
CMD ["./sub-injector"]
