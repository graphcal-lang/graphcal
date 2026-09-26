#!/usr/bin/env nu
# Ratcheted complexity/invariant metrics for the invariant-complexity refactor.
#
# Each metric counts a pattern that the refactor plan wants to drive to zero
# (see internals/refactor-metrics.md). `check` fails when any metric exceeds
# its baseline in internals/refactor-metrics-baseline.toml and asks for the
# baseline to be lowered when a metric improves, so counts only ever go down.
#
# Usage:
#   nu internals/refactor-metrics.nu            # print current counts
#   nu internals/refactor-metrics.nu check      # compare against the baseline
#   nu internals/refactor-metrics.nu update     # rewrite the baseline

const BASELINE = "internals/refactor-metrics-baseline.toml"

# Production Rust sources of the given crates: `tests.rs`, `tests/` trees, and
# inline `#[cfg(test)] mod … {` blocks (and everything after them) are excluded.
# Comment lines are dropped so doc references are not counted.
def production-sources [crates: list<string>]: nothing -> table<path: string, text: string> {
    $crates
    | each {|krate| glob $"crates/($krate)/src/**/*.rs" }
    | flatten
    | where {|path| ($path | path basename) != "tests.rs" and not ($path | str contains "/tests/") }
    | sort
    | each {|path|
        let lines = open --raw $path | lines
        let test_start = $lines
            | enumerate
            | window 2
            | where {|pair| ($pair.0.item | str trim) == "#[cfg(test)]" and ($pair.1.item | str trim) =~ '^(pub(\(\w+\))? )?mod \w+ \{$' }
            | get 0?.0.index
        let code = match $test_start {
            null => $lines
            $index => ($lines | first $index)
        }
        let text = $code | where {|line| not ($line | str trim | str starts-with "//") } | str join "\n"
        {path: ($path | path relative-to $env.PWD), text: $text}
    }
}

def count-matches [sources: table, pattern: string]: nothing -> int {
    $sources | each {|source| $source.text | parse --regex $pattern | length } | math sum
}

def graphcal-error-variants []: nothing -> int {
    let text = open --raw crates/graphcal-compiler/src/registry/error.rs
    let body = $text
        | parse --regex '(?s)pub enum GraphcalError \{(?<body>.*?)\n\}'
        | get 0.body
    $body | lines | where {|line| $line =~ '^    [A-Z][A-Za-z0-9]*\b' } | length
}

def reading-order-sccs []: nothing -> int {
    uv run --quiet internals/reading-order.py
    | lines
    | where {|line| $line | str starts-with "  cycle: " }
    | length
}

def measure []: nothing -> record {
    let core = production-sources [graphcal-compiler graphcal-eval]
    let consumers = production-sources [graphcal-compiler graphcal-eval graphcal-lsp]
    let outside_resolver = $consumers
        | where {|source| not ($source.path | str contains "syntax/module_resolve") }
    let resolver = $core | where {|source| $source.path | str contains "syntax/module_resolve" }
    {
        internal_error_calls: (count-matches $core '(?<!fn )\binternal_error\(')
        resolved_name_from_def_outside_resolver: (count-matches $outside_resolver 'Resolved[A-Za-z]*Name::from_def\b')
        expect_valid_format: (count-matches $core 'expect_valid\(\s*&?format!\(')
        too_many_arguments_expects: (count-matches $consumers 'clippy::too_many_arguments')
        build_declared_types_calls: (count-matches $core '\.build_declared_types\(')
        module_resolve_str_key_lookups: (count-matches $resolver '\.(?:get|get_mut|contains_key|remove)\(\s*[^()]*(?:\.as_str\(\)|\.as_ref\(\)|&\*)')
        pipeline_layers_exceptions: (open internals/pipeline-layers/baseline.toml | get exception | length)
        reading_order_sccs: (reading-order-sccs)
        graphcal_error_variants: (graphcal-error-variants)
    }
}

def main [] {
    measure | transpose metric count | print
}

# Fail when any metric exceeds its baseline or improves without a baseline update.
def "main check" [] {
    let current = measure
    let baseline = open $BASELINE
    let rows = $current
        | transpose metric count
        | each {|row| $row | insert baseline ($baseline | get -o $row.metric) }
    $rows | print
    let missing = $rows | where {|row| $row.baseline == null }
    let regressed = $rows | where {|row| $row.baseline != null and $row.count > $row.baseline }
    let improved = $rows | where {|row| $row.baseline != null and $row.count < $row.baseline }
    let stale = $baseline | columns | where {|metric| $metric not-in ($current | columns) }
    if ($missing | is-not-empty) or ($stale | is-not-empty) {
        error make {msg: $"baseline metrics out of sync: missing ($missing.metric), stale ($stale)"}
    }
    if ($regressed | is-not-empty) {
        error make {msg: $"metrics regressed: ($regressed.metric | str join ', ')"}
    }
    if ($improved | is-not-empty) {
        error make {msg: $"metrics improved; lower the baseline with `nu internals/refactor-metrics.nu update`: ($improved.metric | str join ', ')"}
    }
}

# Rewrite the baseline with the current counts.
def "main update" [] {
    measure | to toml | save --force $BASELINE
}
