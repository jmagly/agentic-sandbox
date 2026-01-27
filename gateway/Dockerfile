# Multi-stage build for auth injection gateway
FROM golang:1.24-alpine AS builder

# Install build dependencies
RUN apk add --no-cache git ca-certificates

WORKDIR /build

# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download

# Copy source code
COPY *.go ./

# Build static binary
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build \
    -ldflags='-w -s -extldflags "-static"' \
    -o gateway \
    .

# Run tests during build
RUN go test -v

# Final stage - minimal runtime image
FROM alpine:latest

# Install CA certificates for upstream HTTPS
RUN apk --no-cache add ca-certificates tzdata && \
    adduser -D -u 1000 -h /app gateway

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/gateway /usr/local/bin/gateway

# Copy default config (can be overridden via volume)
COPY gateway.yaml /etc/gateway/gateway.yaml

# Run as non-root
USER gateway

# Expose default port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

# Entry point
ENTRYPOINT ["gateway"]
CMD ["-config", "/etc/gateway/gateway.yaml"]
