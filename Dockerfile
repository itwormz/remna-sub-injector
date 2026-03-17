FROM alpine:latest

RUN addgroup -S app && adduser -S app -G app
WORKDIR /app
COPY bin/sub-injector ./

USER app
EXPOSE 3020
CMD ["./sub-injector"]
