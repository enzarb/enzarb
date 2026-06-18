FROM golang:1.26-alpine AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /enzarb-operator ./operator/cmd/operator/

FROM gcr.io/distroless/static:nonroot
COPY --from=builder /enzarb-operator /enzarb-operator
USER nonroot:nonroot
ENTRYPOINT ["/enzarb-operator"]
