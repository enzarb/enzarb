FROM golang:1.26.4-alpine AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/root/go/pkg/mod \
    go mod download
COPY . .
RUN --mount=type=cache,target=/root/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -o /enzarb-billing ./billing/cmd/billing/

FROM gcr.io/distroless/static:nonroot
COPY --from=builder /enzarb-billing /enzarb-billing
USER nonroot:nonroot
ENTRYPOINT ["/enzarb-billing"]
