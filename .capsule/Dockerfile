FROM capsule

# Install rustup and stable toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path

ENV PATH="/home/claude/.cargo/bin:${PATH}"

# Persist cargo registry across container runs via the bind-mounted workspace.
# .cargo-cache/ is gitignored in the repo root.
ENV CARGO_HOME=/workspace/.cargo-cache
