FROM rust:1.98-bookworm AS source
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY skills ./skills

FROM source AS build
RUN cargo build --release --locked

FROM source AS windows-build
RUN apt-get update && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu
ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
ENV CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
RUN cargo build --release --locked --target x86_64-pc-windows-gnu

FROM scratch AS windows-artifact
COPY --from=windows-build /build/target/x86_64-pc-windows-gnu/release/agent-mail.exe /agent-mail.exe

FROM scratch AS linux-artifact
COPY --from=build /build/target/release/agent-mail /agent-mail

# Portable Linux binary: toolchain layers remain cached when application sources change.
FROM rust:1.98-bookworm AS linux-static-build
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY skills ./skills
ENV CC_x86_64_unknown_linux_musl=musl-gcc
RUN cargo build --release --locked --target x86_64-unknown-linux-musl

FROM scratch AS linux-static-artifact
COPY --from=linux-static-build /build/target/x86_64-unknown-linux-musl/release/agent-mail /agent-mail
