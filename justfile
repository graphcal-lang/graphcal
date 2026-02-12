lint:
    RUSTFLAGS="-Dwarnings" cargo clippy --workspace --all-targets --all-features
    RUSTFLAGS="-Dwarnings" cargo clippy --workspace --all-targets --no-default-features
    cargo fmt --check
    RUSTDOCFLAGS="-Dwarnings" cargo doc --workspace --no-deps
    cargo check --workspace

test:
    cargo test --workspace

coverage:
    cargo llvm-cov --workspace --html
    @echo "Coverage report generated at target/llvm-cov/html/index.html"

coverage-open:
    cargo llvm-cov --workspace --html --open

coverage-lcov:
    cargo llvm-cov --workspace --lcov --output-path lcov.info

coverage-clean:
    cargo llvm-cov clean --workspace
