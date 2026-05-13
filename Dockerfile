FROM rust:1.95-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

FROM public.ecr.aws/lambda/provided:al2023
WORKDIR /var/task
COPY --from=builder /app/target/release/tokenrouter ./bootstrap
COPY frontend/dist ./frontend/dist
ENV RUST_LOG=info
CMD ["bootstrap"]
