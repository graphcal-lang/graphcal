#!/usr/bin/env nu
# Ratcheted complexity/invariant metrics for the invariant-complexity refactor.
#
# Each metric counts a pattern that the refactor plan wants to drive to zero
# (see internals/refactor-metrics.md). `check` fails when any metric exceeds
# its baseline in internals/refactor-metrics-baseline.toml and asks for the
# baseline to be lowered when a metric improves, so counts only ever go down.
#
# Usage:
#   nu internals/refactor-metrics.nu                # print current counts
#   nu internals/refactor-metrics.nu check          # compare against the baseline
#   nu internals/refactor-metrics.nu update         # rewrite the baseline
#   nu internals/refactor-metrics.nu list [metric]  # list the counted sites

const BASELINE = "internals/refactor-metrics-baseline.toml"
const LAYERS_BASELINE = "internals/pipeline-layers/baseline.toml"

# Production Rust sources of the given crates: `tests.rs`, `tests/` trees, and
# inline `#[cfg(test)] mod … {` blocks (and everything after them) are excluded.
# Comment lines are dropped so doc references are not counted; `lines` keeps
# every remaining line with its 1-based line number in the file.
def production-sources [crates: list<string>]: nothing -> table<path: string, lines: table<line: int, item: string>> {
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
        let kept = $code
            | enumerate
            | where {|line| not ($line.item | str trim | str starts-with "//") }
            | each {|line| {line: ($line.index + 1), item: $line.item} }
        {path: ($path | path relative-to $env.PWD), lines: $kept}
    }
}

# Every match of `pattern` in the sources, one row per match, located at the
# line where the match starts. Matching runs on the joined code lines, so a
# pattern may span lines; a NUL sentinel inserted before each match recovers
# the line it starts on.
def match-sites [sources: table, pattern: string]: nothing -> table<site: string, text: string> {
    $sources
    | each {|source|
        let marked = $source.lines.item
            | str join "\n"
            | str replace --all --regex $pattern "\u{0}$0"
            | split row "\n"
        $source.lines
        | zip $marked
        | each {|pair|
            let hits = ($pair.1 | split row "\u{0}" | length) - 1
            let site = {site: $"($source.path):($pair.0.line)", text: ($pair.0.item | str trim)}
            if $hits == 0 { [] } else { 1..$hits | each {|_| $site } }
        }
        | flatten
    }
    | flatten
}

def pipeline-layers-exceptions []: nothing -> table<site: string, text: string> {
    open $LAYERS_BASELINE
    | default [] exception
    | get exception
    | each {|exception| {site: $LAYERS_BASELINE, text: ($exception | to nuon)} }
}

def reading-order-cycles []: nothing -> table<site: string, text: string> {
    uv run --quiet internals/reading-order.py
    | lines
    | where {|line| $line | str starts-with "  cycle: " }
    | each {|line| {site: "internals/reading-order.py", text: ($line | str trim)} }
}

# Every metric with a closure that lists its counted sites. A metric's count is
# the length of that list, so `list` shows exactly what the counts count.
def metrics []: nothing -> table<metric: string, sites: closure> {
    let core = production-sources [graphcal-compiler graphcal-eval graphcal-project]
    let consumers = production-sources [graphcal-compiler graphcal-eval graphcal-project graphcal-lsp]
    let outside_resolver = $consumers
        | where {|source| not ($source.path | str contains "graphcal-compiler/src/resolve/") }
    let semantic_families = $core
        | where {|source| $source.path | str contains "graphcal-compiler/src/semantic_error/" }
    let resolver = $core | where {|source| $source.path | str contains "graphcal-compiler/src/resolve/" }
    [
        {metric: internal_error_calls, sites: {|| match-sites $core '(?<!fn )\b(?:\w*(?:internal|invariant)\w*|Invariant::violated|InternalError::new)\(' }}
        {metric: resolved_name_from_def_outside_resolver, sites: {|| match-sites $outside_resolver 'Resolved[A-Za-z]*Name::from_def\b' }}
        {metric: expect_valid_format, sites: {|| match-sites $core 'expect_valid\(\s*&?format!\(' }}
        {metric: too_many_arguments_expects, sites: {|| match-sites $consumers 'clippy::too_many_arguments' }}
        {metric: build_declared_types_calls, sites: {|| match-sites $core '\.build_declared_types\(' }}
        {metric: module_resolve_str_key_lookups, sites: {|| match-sites $resolver '\.(?:get|get_mut|contains_key|remove)\(\s*[^()]*(?:\.as_str\(\)|\.as_ref\(\)|&\*)' }}
        {metric: pipeline_layers_exceptions, sites: {|| pipeline-layers-exceptions }}
        {metric: reading_order_sccs, sites: {|| reading-order-cycles }}
        {metric: semantic_error_string_payloads, sites: {|| match-sites $semantic_families '\b[a-z_][a-z0-9_]*: String\b' }}
    ]
}

def measure []: nothing -> record {
    metrics
    | each {|row| {metric: $row.metric, count: (do $row.sites | length)} }
    | transpose --header-row --as-record
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

# Print every counted site as `path:line: <matched line>`, grouped per metric
# (or for the given metric only); each group's length equals the metric's count.
def "main list" [
    metric?: string # the metric to list; all metrics when omitted
] {
    let selected = metrics | where {|row| $metric == null or $row.metric == $metric }
    if ($selected | is-empty) {
        error make {msg: $"unknown metric: ($metric)"}
    }
    for row in $selected {
        let sites = do $row.sites
        print $"## ($row.metric) \(($sites | length)\)"
        for site in $sites {
            print $"($site.site): ($site.text)"
        }
        print ""
    }
}
