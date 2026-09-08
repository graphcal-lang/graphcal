# Build all reviewed Lean specifications and reject placeholders or local axioms.
formal:
    cd formal && lake build --wfail
    @if rg --line-number '\b(sorry|admit|axiom)\b' formal --glob '*.lean'; then echo 'formal specifications must not contain sorry, admit, or custom axioms' >&2; exit 1; fi

# Compare production Rust with the reviewed finite Lean oracle matrices.
formal-conformance: formal
    GRAPHCAL_REQUIRED_BINDABILITY_ORACLE="$(pwd)/formal/.lake/build/bin/required-bindability-oracle" cargo test --package graphcal-compiler --lib required_bindability_matches_lean_oracle -- --ignored
    GRAPHCAL_TEMPLATE_CLOSURE_ORACLE="$(pwd)/formal/.lake/build/bin/template-closure-oracle" cargo test --package graphcal-compiler --lib template_closure_matches_lean_oracle -- --ignored
    GRAPHCAL_NAMESPACE_RESOLUTION_ORACLE="$(pwd)/formal/.lake/build/bin/namespace-resolution-oracle" cargo test --package graphcal-eval --test namespace_formal_conformance -- --ignored
    GRAPHCAL_EXTERNAL_SURFACE_ORACLE="$(pwd)/formal/.lake/build/bin/external-surface-oracle" cargo test --package graphcal-compiler --lib external_surface_matches_lean_oracle -- --ignored

# Syntax-aware module roles, exact dependency debt, and fail-closed fixtures.
pipeline-layers:
    cargo test --locked --manifest-path internals/pipeline-layers/Cargo.toml
    cargo run --locked --manifest-path internals/pipeline-layers/Cargo.toml -- check .

pipeline-layers-lint: pipeline-layers
    cargo audit --deny warnings --file internals/pipeline-layers/Cargo.lock
    cargo clippy --locked --manifest-path internals/pipeline-layers/Cargo.toml --all-targets -- -D warnings
    cargo fmt --manifest-path internals/pipeline-layers/Cargo.toml --check

# Stage the single-source example catalog, Wasm engine, and vendored plot runtime.
playground-assets:
    vp -C web/playground exec node stage-examples.mjs
    wasm-pack build crates/graphcal-wasm --target web --out-dir ../../web/playground/public/pkg --profile wasm-release --no-typescript --no-pack
    mkdir -p web/playground/public/vega
    cp crates/graphcal-report/assets/vega.min.js crates/graphcal-report/assets/vega-lite.min.js crates/graphcal-report/assets/vega-embed.min.js web/playground/public/vega/

playground-build: playground-assets playground-check playground-test
    cd web/playground && vp build

playground-serve: playground-assets
    cd web/playground && vp dev

playground-check:
    cd web/playground && vp check

playground-test: playground-assets
    cd web/playground && vp test

lint: formal pipeline-layers-lint playground-check
    cargo audit --deny warnings
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --no-default-features
    cargo fmt --check
    cargo metadata --locked --manifest-path fuzz/Cargo.toml --format-version 1 > /dev/null
    CARGO_BUILD_WARNINGS=deny cargo doc --workspace --no-deps
    CARGO_BUILD_WARNINGS=deny cargo check --workspace

test: formal-conformance pipeline-layers playground-test
    cargo test --workspace
    node internals/report-runtime-tests.mjs

# Audit the closed-world CLI surface while preserving documented external crate
# and ABI boundaries that Hawk cannot observe from the shipped binary.
# Baseline: zero findings with cargo-hawk 0.1.13 on Rust 1.98.0 (2026-08-30).
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
        --exclude-crate graphcal_test_support \
        --exclude-crate graphcal_wasm

wasm-test:
    wasm-pack test --node crates/graphcal-wasm

# Build the no-modules engine bundle embedded in hydrated reports.
wasm-report:
    rm -rf target/wasm-report/pkg
    wasm-pack build crates/graphcal-wasm --target no-modules --out-dir ../../target/wasm-report/pkg --profile wasm-release --no-typescript --no-pack
    rm -f target/wasm-report/pkg/.gitignore

# Regenerate the release-matched browser engine embedded in the CLI.
wasm-report-update: wasm-report
    mkdir -p crates/graphcal-cli/assets/report-engine
    cp target/wasm-report/pkg/graphcal_wasm.js target/wasm-report/pkg/graphcal_wasm_bg.wasm crates/graphcal-cli/assets/report-engine/
    GRAPHCAL_UPDATE_REPORT_ENGINE=1 cargo check -p graphcal

# Explicitly verify the check that every source-checkout CLI build performs.
wasm-report-check:
    cargo check -p graphcal

# Real file:// hydration and Vega regression tests in an isolated Chrome profile.
# Set GRAPHCAL_CHROME when Chrome is not installed at the standard macOS path.
report-browser-test:
    cargo build -p graphcal
    node internals/report-browser-tests.mjs

# Build a hydrated demo report with the embedded engine and drive its payload
# through the prepared-project API under Node.
report-smoke: wasm-report wasm-report-check
    cargo run -p graphcal -- report build tests/fixtures/valid/rocket.gcl --output target/wasm-report/rocket.report.html
    node internals/report-hydration-smoke.mjs target/wasm-report/rocket.report.html

# Assemble only after Zensical's clean build, preserving both sibling apps.
docs-assemble:
    vp -C web/playground exec node ../../internals/site-assemble.mjs
    vp -C web/playground exec node ../../internals/site-verify.mjs

# Build and verify the entire GitHub Pages artifact with the same steps as CI.
docs-build: playground-build
    zensical build --clean
    just docs-assemble

# Documentation-only development; use site-serve to preview /playground/ too.
docs-serve:
    zensical serve

site-serve: docs-build
    vp -C web/playground exec node ../../internals/site-serve.mjs

coverage:
    cargo llvm-cov --workspace --html
    @echo "Coverage report generated at target/llvm-cov/html/index.html"

coverage-open:
    cargo llvm-cov --workspace --html --open

coverage-lcov:
    cargo llvm-cov --workspace --lcov --output-path lcov.info

coverage-clean:
    cargo llvm-cov clean --workspace
