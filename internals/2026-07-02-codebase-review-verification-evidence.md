# Verification evidence for 2026-07-02 codebase review fixes

Date: 2026-07-03

This records the command outputs used to verify that every finding in `.local/2026-07-02_codebase-review.md` is represented in the resolution checklist, that every referenced resolution commit is present in the applied GitButler stack, and that the workspace verifies successfully.

## 1. Finding-ID coverage is exact

Command:

```sh
tmpdir=$(mktemp -d)
{ rg -o '^### (H[0-9]+)' -r '$1' .local/2026-07-02_codebase-review.md; \
  rg -o '^- \*\*([MLS][0-9]+)\.' -r '$1' .local/2026-07-02_codebase-review.md; } | sort -V > "$tmpdir/review.ids"
rg -o '^\| (H[0-9]+|M[0-9]+|L[0-9]+|S[0-9]+) \|' -r '$1' \
  internals/2026-07-02-codebase-review-resolution-checklist.md | sort -V > "$tmpdir/checklist.ids"
printf 'review_count=%s\n' "$(wc -l < "$tmpdir/review.ids" | tr -d ' ')"
printf 'checklist_count=%s\n' "$(wc -l < "$tmpdir/checklist.ids" | tr -d ' ')"
printf 'review_by_prefix='; cut -c1 "$tmpdir/review.ids" | sort | uniq -c | tr '\n' ' '; printf '\n'
printf 'checklist_by_prefix='; cut -c1 "$tmpdir/checklist.ids" | sort | uniq -c | tr '\n' ' '; printf '\n'
printf 'missing_from_checklist='; missing=$(comm -23 "$tmpdir/review.ids" "$tmpdir/checklist.ids" | paste -sd, -); printf '%s\n' "${missing:-none}"
printf 'extra_in_checklist='; extra=$(comm -13 "$tmpdir/review.ids" "$tmpdir/checklist.ids" | paste -sd, -); printf '%s\n' "${extra:-none}"
printf 'review_ids='; paste -sd' ' "$tmpdir/review.ids"; printf '\n'
```

Output:

```text
review_count=84
checklist_count=84
review_by_prefix=   4 H   33 L   20 M   27 S 
checklist_by_prefix=   4 H   33 L   20 M   27 S 
missing_from_checklist=none
extra_in_checklist=none
review_ids=H1 H2 H3 H4 L1 L2 L3 L4 L5 L6 L7 L8 L9 L10 L11 L12 L13 L14 L15 L16 L17 L18 L19 L20 L21 L22 L23 L24 L25 L26 L27 L28 L29 L30 L31 L32 L33 M1 M2 M3 M4 M5 M6 M7 M8 M9 M10 M11 M12 M13 M14 M15 M16 M17 M18 M19 M20 S1 S2 S3 S4 S5 S6 S7 S8 S9 S10 S11 S12 S13 S14 S15 S16 S17 S18 S19 S20 S21 S22 S23 S24 S25 S26 S27
```

## 2. Referenced resolution commits are in the applied GitButler stack

Command:

```sh
tmpdir=$(mktemp -d)
rg -o '`[0-9a-f]{7,8}`' internals/2026-07-02-codebase-review-resolution-checklist.md \
  | tr -d '`' | sort -u > "$tmpdir/expected.commits"
but status > "$tmpdir/but.status"
rg -o '[0-9a-f]{8}' "$tmpdir/but.status" | sort -u > "$tmpdir/stack.commits"
printf 'expected_resolution_commits=%s\n' "$(wc -l < "$tmpdir/expected.commits" | tr -d ' ')"
printf 'stack_commits_seen_by_but_status=%s\n' "$(wc -l < "$tmpdir/stack.commits" | tr -d ' ')"
printf 'missing_resolution_commits_from_stack='; missing=$(comm -23 "$tmpdir/expected.commits" "$tmpdir/stack.commits" | paste -sd, -); printf '%s\n' "${missing:-none}"
printf 'resolution_commits_in_stack='; paste -sd' ' "$tmpdir/expected.commits"; printf '\n'
```

Output:

```text
expected_resolution_commits=53
stack_commits_seen_by_but_status=57
missing_resolution_commits_from_stack=none
resolution_commits_in_stack=008d4c1d 04d8a212 0518e593 0c59acc8 0d083614 0e5f3dc8 19806076 21bb02b5 2a50b45c 2cf34b06 36aad539 3a050595 3bc54a4d 3f08b62c 40188b70 449f6605 492deeb2 4b66dfec 4e12d433 4fc56ace 5068aa52 520733d2 532914b8 554086ae 630fb31d 64da295c 64e546b0 6b9f60ef 733ddb87 7488bd67 7e09b010 83274a68 881f2659 8c645add 8f4c1f44 966a60bd 997b800f 9e13b352 a07a5656 a3fb9811 a95a2dda acdccee3 bd53cc45 be8fd71e c3e5b498 c453d8cb c92f2353 d39e267f dbe31416 de853d8d e59d4bbd efcafddb f5cc378d
```

## 3. Whole-workspace verification passed with non-truncated exit-status output

Command:

```sh
set +e
fmt_log=$(mktemp)
check_log=$(mktemp)
test_log=$(mktemp)
cargo fmt --check >"$fmt_log" 2>&1; fmt_rc=$?
cargo check --workspace --all-targets >"$check_log" 2>&1; check_rc=$?
cargo test --workspace --all-targets >"$test_log" 2>&1; test_rc=$?
printf 'fmt_exit=%s\n' "$fmt_rc"
tail -n 5 "$fmt_log" | sed 's/^/fmt: /'
printf 'check_exit=%s\n' "$check_rc"
{ grep -E '^(    Finished|warning:|error:)' "$check_log" | tail -20; } | sed 's/^/check: /'
printf 'test_exit=%s\n' "$test_rc"
{ grep -E '^(test result:|Doc-tests|running [0-9]+ tests|     Running|   Doc-tests)' "$test_log" | tail -80; } | sed 's/^/test: /'
if [ "$fmt_rc" -eq 0 ] && [ "$check_rc" -eq 0 ] && [ "$test_rc" -eq 0 ]; then
  printf 'overall=passed\n'
else
  printf 'overall=failed\n'
  exit 1
fi
```

Output:

```text
fmt_exit=0
check_exit=0
check:     Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 07s
test_exit=0
test:      Running unittests src/lib.rs (target/debug/deps/graphcal-b849a8172ac4a48c)
test: running 0 tests
test: test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test:      Running unittests src/main.rs (target/debug/deps/graphcal-f9f6aed40f32393d)
test: running 52 tests
test: test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test:      Running tests/cli.rs (target/debug/deps/cli-d5dfefb2581fd14b)
test: running 128 tests
test: test result: ok. 128 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.50s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_compiler-fe385546da5b3391)
test: running 775 tests
test: test result: ok. 775 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_eval-2804fda621761057)
test: running 233 tests
test: test result: ok. 233 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test:      Running tests/declaration_order.rs (target/debug/deps/declaration_order-d772a21f115206de)
test: running 8 tests
test: test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s
test:      Running tests/edge_case_bugs.rs (target/debug/deps/edge_case_bugs-a9034167e96bfdcd)
test: running 40 tests
test: test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test:      Running tests/error_snapshots.rs (target/debug/deps/error_snapshots-bdc1e780b390eeae)
test: running 94 tests
test: test result: ok. 94 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test:      Running tests/phase0_regressions.rs (target/debug/deps/phase0_regressions-6dd893812ad4173a)
test: running 20 tests
test: test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.98s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_fmt-fc1201644a57621d)
test: running 2 tests
test: test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test:      Running tests/format_tests.rs (target/debug/deps/format_tests-d641fd98f3dd0609)
test: running 153 tests
test: test result: ok. 153 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_io-ae9417a0d347b459)
test: running 19 tests
test: test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_lsp-be5e6c78d103e4ea)
test: running 114 tests
test: test result: ok. 114 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test:      Running unittests src/lib.rs (target/debug/deps/graphcal_package-9137133a17df282d)
test: running 17 tests
test: test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
overall=passed
```
