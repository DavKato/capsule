FROM capsule

# gcc provides the C linker required by rustc to link binaries
USER root
RUN apt-get update && apt-get install -y --no-install-recommends gcc && rm -rf /var/lib/apt/lists/*
USER claude

# Install rustup and stable toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path

ENV PATH="/home/claude/.cargo/bin:${PATH}"
