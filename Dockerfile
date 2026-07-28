FROM rust:1.85-slim AS build

WORKDIR /build

# libz3-dev: the z3 feature (on by default, see Cargo.toml) links libz3
# dynamically. libclang-dev: z3-sys's build.rs uses bindgen to generate FFI
# bindings from z3.h, which needs libclang to parse the header.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libz3-dev libclang-dev \
    && rm -rf /var/lib/apt/lists/*

COPY . .

# --no-default-features --features z3 keeps Z3 (needed for `mvl prove`/L5
# refinement obligations) but drops openapi codegen, unneeded in the shipped
# binary. Z3_SYS_Z3_HEADER/LIBRARY_PATH are found dynamically rather than
# hardcoded so the same Dockerfile works on amd64 and arm64 (multiarch lib
# dirs differ: /usr/lib/x86_64-linux-gnu vs /usr/lib/aarch64-linux-gnu).
RUN Z3_LIB_DIR="$(dirname "$(find /usr/lib -name 'libz3.so*' | head -1)")" && \
    Z3_SYS_Z3_HEADER=/usr/include/z3.h LIBRARY_PATH="${Z3_LIB_DIR}" \
        cargo build --release --no-default-features --features z3 && \
    cp target/release/mvl /usr/local/bin/mvl && \
    mkdir -p /opt/mvl/lib && \
    cp "${Z3_LIB_DIR}"/libz3.so* /opt/mvl/lib/

ARG MVL_VERSION
RUN mkdir -p /opt/mvl/toolchains/${MVL_VERSION} && \
    cp -r std /opt/mvl/toolchains/${MVL_VERSION}/std

FROM gcr.io/distroless/cc-debian12

ARG MVL_VERSION
ENV MVL_HOME=/opt/mvl
ENV MVL_VERSION=${MVL_VERSION}
ENV LD_LIBRARY_PATH=/opt/mvl/lib

COPY --from=build /usr/local/bin/mvl /usr/local/bin/mvl
COPY --from=build /opt/mvl /opt/mvl

ENTRYPOINT ["mvl"]
