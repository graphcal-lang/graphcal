# Build all reviewed Lean specifications and reject placeholders or local axioms.
formal:
    cd formal && lake build --wfail
    @if rg --line-number '\b(sorry|admit|axiom)\b' formal --glob '*.lean'; then echo 'formal specifications must not contain sorry, admit, or custom axioms' >&2; exit 1; fi

# Compare production Rust with the reviewed finite Lean oracle matrices.
formal-conformance: formal
    GRAPHCAL_REQUIRED_BINDABILITY_ORACLE="$(pwd)/formal/.lake/build/bin/required-bindability-oracle" cargo test --package graphcal-compiler --lib required_bindability_matches_lean_oracle -- --ignored
    GRAPHCAL_NAMESPACE_RESOLUTION_ORACLE="$(pwd)/formal/.lake/build/bin/namespace-resolution-oracle" cargo test --package graphcal-eval --test namespace_formal_conformance -- --ignored

lint: formal
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --no-default-features
    cargo fmt --check
    CARGO_BUILD_WARNINGS=deny cargo doc --workspace --no-deps
    CARGO_BUILD_WARNINGS=deny cargo check --workspace

test: formal-conformance
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
        --exclude-crate graphcal_tenax \
        --exclude-crate graphcal_test_support

wasm-test:
    wasm-pack test --node crates/graphcal-wasm

# Build the no-modules engine bundle consumed by hydrated reports
# (`graphcal report build` embeds it; see docs/cli-reference.md).
wasm-report:
    rm -rf target/wasm-report/pkg
    wasm-pack build crates/graphcal-wasm --target no-modules --out-dir ../../target/wasm-report/pkg --profile wasm-release --no-typescript --no-pack
    rm -f target/wasm-report/pkg/.gitignore

# Build a hydrated demo report with the real engine and drive its embedded
# payload through the prepared-project API under Node.
report-smoke: wasm-report
    cargo run -p graphcal -- report build tests/fixtures/valid/rocket.gcl --output target/wasm-report/rocket.report.html
    node internals/report-hydration-smoke.mjs target/wasm-report/rocket.report.html

# Build the browser adapter and wasm-bindgen glue consumed by Zensical, and
# stage the vendored Vega bundles the playground loads for plot rendering.
wasm-playground:
    rm -rf docs/assets/playground/pkg
    wasm-pack build crates/graphcal-wasm --target web --out-dir ../../docs/assets/playground/pkg --profile wasm-release --no-typescript --no-pack
    rm -f docs/assets/playground/pkg/.gitignore
    rm -rf docs/assets/playground/vega
    mkdir -p docs/assets/playground/vega
    cp crates/graphcal-report/assets/vega.min.js crates/graphcal-report/assets/vega-lite.min.js crates/graphcal-report/assets/vega-embed.min.js docs/assets/playground/vega/

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
    test -s site/docs/assets/playground/vega/vega.min.js
    test -s site/docs/assets/playground/vega/vega-lite.min.js
    test -s site/docs/assets/playground/vega/vega-embed.min.js
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
