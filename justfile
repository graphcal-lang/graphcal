lint:
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --no-default-features
    cargo fmt --check
    CARGO_BUILD_WARNINGS=deny cargo doc --workspace --no-deps
    CARGO_BUILD_WARNINGS=deny cargo check --workspace

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
