# Build context: repository root.
FROM golang:1.27-alpine AS build
WORKDIR /src
COPY server/go.mod server/go.sum ./
RUN go mod download
COPY server/ ./
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/server ./cmd/server

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/server /app/server
COPY shared /app/shared
ENV FF_SHARED_DIR=/app/shared
EXPOSE 4002/tcp 4003/udp
ENTRYPOINT ["/app/server"]
