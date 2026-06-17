FROM golang:1.24-alpine AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /enzarb-billing ./billing/cmd/billing/

FROM gcr.io/distroless/static:nonroot
COPY --from=builder /enzarb-billing /enzarb-billing
ENTRYPOINT ["/enzarb-billing"]
