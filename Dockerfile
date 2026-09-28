# syntax=docker/dockerfile:1
FROM rust:1.85-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p agentmesh

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/release/agentmesh /usr/local/bin/agentmesh
COPY config/agentmesh.yaml /etc/agentmesh/agentmesh.yaml
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/agentmesh"]
CMD ["serve", "--config", "/etc/agentmesh/agentmesh.yaml"]

