
# =====================================================
# STAGE 1: Build the optimized Rust engine
# =====================================================

FROM rust:1-bookworm AS rust_builder

WORKDIR /build

COPY rust_engine/Cargo.toml rust_engine/Cargo.lock /build/rust_engine/
COPY rust_engine/src/ /build/rust_engine/src/

RUN cargo build \
    --locked \
    --release \
    --manifest-path /build/rust_engine/Cargo.toml

# =====================================================
# STAGE 2: Python + DuckDB runtime
# =====================================================

FROM python:3.12-slim-bookworm

RUN useradd \
    --uid 1000 \
    --no-create-home \
    --home-dir /scratch \
    app

WORKDIR /app

COPY requirements.txt /app/requirements.txt

RUN pip install \
    --no-cache-dir \
    -r /app/requirements.txt

COPY --from=rust_builder \
    /build/rust_engine/target/release/rust_engine \
    /app/rust_engine

COPY pipeline/ /app/pipeline/
COPY run.sh /app/run.sh

# Remove Windows BOM and CRLF from run.sh.
RUN python3 -c 'from pathlib import Path; p=Path("/app/run.sh"); data=p.read_bytes().removeprefix(bytes.fromhex("efbbbf")).replace(b"\r\n", b"\n"); p.write_bytes(data)' \
    && chmod 755 /app/run.sh /app/rust_engine

ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

# IMPORTANT:
# Start the script explicitly with the Linux shell.
CMD ["/bin/sh", "/app/run.sh"]