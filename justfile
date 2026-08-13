lint:
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --no-default-features
    cargo fmt --check
    CARGO_BUILD_WARNINGS=deny cargo doc --workspace --no-deps
    CARGO_BUILD_WARNINGS=deny cargo check --workspace

test:
    cargo test --workspace

# Audit the closed-world CLI surface while preserving documented external crate
# and ABI boundaries that Hawk cannot observe from the shipped binary.
# Baseline: zero findings with cargo-hawk 0.1.9 on Rust 1.97.1 (2026-08-14).
hawk:
    cargo hawk check -D warnings \
        --exclude-crate graphcal_compiler \
        --exclude-crate graphcal_eval \
        --exclude-crate graphcal_io \
        --exclude-crate graphcal_package \
        --exclude-crate graphcal_plugin \
        --exclude-crate graphcal_plugin_abi \
        --exclude-crate graphcal_plugin_host \
        --exclude-crate graphcal_tenax

wasm-test:
    wasm-pack test --node crates/graphcal-wasm

# Build the browser adapter and wasm-bindgen glue consumed by Zensical.
wasm-playground:
    rm -rf docs/assets/playground/pkg
    wasm-pack build crates/graphcal-wasm --target web --out-dir ../../docs/assets/playground/pkg --release --no-typescript --no-pack
    rm -f docs/assets/playground/pkg/.gitignore

# Populate the published site root: redirect stub, CNAME, robots.txt, and a
# copy of the themed 404 page that GitHub Pages serves for unmatched paths.
docs-assemble:
    cp -R site-root/. site/
    cp site/docs/404.html site/404.html

# Build docs, smoke-test the worker bridge, and enforce a 5 MiB raw Wasm budget.
docs-build: wasm-playground
    zensical build --clean
    just docs-assemble
    test -s site/docs/assets/playground/pkg/graphcal_wasm.js
    test -s site/docs/assets/playground/pkg/graphcal_wasm_bg.wasm
    test "$(wc -c < site/docs/assets/playground/pkg/graphcal_wasm_bg.wasm)" -le 5242880
    node --check docs/javascripts/playground.mjs
    node --check docs/javascripts/playground-worker.mjs
    node internals/playground-worker-smoke.mjs

docs-serve: wasm-playground
    zensical serve

coverage:
    cargo llvm-cov --workspace --html
    @echo "Coverage report generated at target/llvm-cov/html/index.html"

coverage-open:
    cargo llvm-cov --workspace --html --open

coverage-lcov:
    cargo llvm-cov --workspace --lcov --output-path lcov.info

coverage-clean:
    cargo llvm-cov clean --workspace
