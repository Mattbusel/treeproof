#!/usr/bin/env bash
# Run treeproof over real projects for each supported runner and check the parsed counts.
# Needs: treeproof on PATH, cargo (+ cargo-nextest), python3 with pytest and pytest-xdist, go, node/npm.
set -euo pipefail

work="$(mktemp -d)"
export TREEPROOF_CACHE_DIR="$work/cache" TREEPROOF_KEY_FILE="$work/key"
fail=0

new_repo() {
  rm -rf "$work/$1"; mkdir -p "$work/$1"; cd "$work/$1"
  git init -q -b main
  git config user.email ci@example.com; git config user.name ci
}

# expect <label> <want-result> <want-passed> <want-failed> -- cmd...
expect() {
  local label="$1" want="$2" wp="$3" wf="$4"; shift 5
  local out
  out="$(treeproof run --no-cache --json -- "$@" 2>&1 >/dev/null || true)"
  local got gp gf
  got="$(printf '%s\n' "$out" | sed -n 's/^treeproof: fresh \([A-Z]*\) .*/\1/p' | head -1)"
  gp="$(printf '%s\n' "$out" | sed -n 's/.*(\([0-9]*\) passed, \([0-9]*\) failed.*/\1/p' | head -1)"
  gf="$(printf '%s\n' "$out" | sed -n 's/.*(\([0-9]*\) passed, \([0-9]*\) failed.*/\2/p' | head -1)"
  if [ "$got" = "$want" ] && [ "$gp" = "$wp" ] && [ "$gf" = "$wf" ]; then
    echo "ok   $label: $got $gp passed $gf failed"
  else
    echo "FAIL $label: got '$got' $gp/$gf, want $want $wp/$wf"; printf '%s\n' "$out" | tail -20
    fail=1
  fi
}

# cargo test and nextest
new_repo rust
cargo init -q --lib --name demo
cat > src/lib.rs <<'RS'
pub fn add(a: i32, b: i32) -> i32 { a + b }
#[cfg(test)]
mod tests {
    #[test] fn adds() { assert_eq!(super::add(1, 2), 3); }
    #[test] fn adds_neg() { assert_eq!(super::add(-1, -2), -3); }
    #[test] #[ignore] fn slow() {}
    #[test] fn env_dependent() { assert!(std::env::var("BREAK").is_err()); }
}
RS
printf 'target/\n' > .gitignore
cargo generate-lockfile -q
expect "cargo test" PASSED 3 0 -- cargo test
BREAK=1 expect "cargo test failing" FAILED 2 1 -- cargo test
expect "cargo test filtered to nothing" FAILED 0 0 -- cargo test no_such_test
expect "cargo nextest" PASSED 3 0 -- cargo nextest run
BREAK=1 expect "cargo nextest failing" FAILED 2 1 -- cargo nextest run

# pytest and xdist
new_repo py
printf '__pycache__/\n.pytest_cache/\n' > .gitignore
cat > test_demo.py <<'PY'
import os, pytest
def test_a(): assert 1
def test_b(): assert os.environ.get("BREAK") is None
@pytest.mark.skip
def test_c(): pass
PY
expect "pytest" PASSED 2 0 -- python3 -m pytest
expect "pytest -v" PASSED 2 0 -- python3 -m pytest -v
BREAK=1 expect "pytest failing" FAILED 1 1 -- python3 -m pytest
expect "pytest xdist" PASSED 2 0 -- python3 -m pytest -n 2 -v
expect "pytest deselect all" FAILED 0 0 -- python3 -m pytest -k nothing
expect "pytest collect-only" FAILED 0 0 -- python3 -m pytest --collect-only

# go test
new_repo go
go mod init example.com/demo >/dev/null 2>&1
cat > demo_test.go <<'GO'
package demo
import ("os"; "testing")
func TestA(t *testing.T) {}
func TestB(t *testing.T) { if os.Getenv("BREAK") != "" { t.Fatal("broken") } }
func TestSkip(t *testing.T) { t.Skip("skip") }
GO
expect "go test -v" PASSED 2 0 -- go test -v ./...
expect "go test -json" PASSED 2 0 -- go test -json ./...
BREAK=1 expect "go test failing" FAILED 1 1 -- go test -v ./...
expect "go test packages" PASSED 1 0 -- go test -count=1 ./...
expect "go test run nothing" FAILED 0 0 -- go test -run Nope ./...

# jest and vitest
new_repo js
printf 'node_modules/\n' > .gitignore
npm init -y >/dev/null
npm i -D jest vitest --no-audit --no-fund --silent >/dev/null 2>&1
mkdir -p j v
cat > j/demo.test.js <<'JS'
test('a', () => { expect(1).toBe(1); });
test('b', () => { expect(process.env.BREAK).toBeUndefined(); });
test.skip('c', () => {});
JS
cat > v/demo.test.mjs <<'JS'
import { test, expect } from 'vitest';
test('a', () => { expect(1).toBe(1); });
test('b', () => { expect(process.env.BREAK).toBeUndefined(); });
test.skip('c', () => {});
JS
expect "jest" PASSED 2 0 -- npx jest --rootDir j
BREAK=1 expect "jest failing" FAILED 1 1 -- npx jest --rootDir j
expect "vitest" PASSED 2 0 -- npx vitest run v
BREAK=1 expect "vitest failing" FAILED 1 1 -- npx vitest run v
expect "vitest no files" FAILED 0 0 -- npx vitest run nothing_here

exit $fail
