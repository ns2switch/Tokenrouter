FROM node:22-bookworm AS frontend-builder
WORKDIR /app
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.95-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

FROM public.ecr.aws/lambda/provided:al2023
WORKDIR /var/task
COPY --from=builder /app/target/release/tokenrouter ./bootstrap
COPY --from=frontend-builder /app/dist ./frontend/dist
ENV RUST_LOG=info
CMD ["bootstrap"]
