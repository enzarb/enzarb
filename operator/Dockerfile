FROM golang:1.26.4-alpine AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/root/go/pkg/mod \
    go mod download
COPY . .
RUN --mount=type=cache,target=/root/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -o /enzarb-operator ./cmd/operator/

FROM gcr.io/distroless/static:nonroot
COPY --from=builder /enzarb-operator /enzarb-operator
USER nonroot:nonroot
ENTRYPOINT ["/enzarb-operator"]
