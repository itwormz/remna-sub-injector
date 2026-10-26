# Packaging only: CI supplies the tested release binary for this architecture.
FROM alpine:3.23
RUN apk add --no-cache ca-certificates \
    && addgroup -S app && adduser -S app -G app
LABEL org.opencontainers.image.source="https://github.com/itwormz/remna-sub-injector"
WORKDIR /app
ARG BINARY
COPY --chmod=755 ${BINARY} /app/sub-injector
USER app
EXPOSE 3020
CMD ["./sub-injector"]
