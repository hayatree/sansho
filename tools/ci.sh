#!/usr/bin/env bash
# Local CI: the checks every change must pass before it is merged to main.
# Provenance: original script for this repository.
#
# Usage: tools/ci.sh [--quick|--cached]
#   --cached  run full CI offline with the local ngspice reference cache
#   --quick  only fmt, clippy (default features) and tests (default features)
#
# GitHub Actions runs only on pushes to main (and manual runs) to save minutes;
# this script is the primary gate. It is a superset of the GitHub workflow: it
# also checks the `jit` feature and the Verilog-A decks (skipped when the local
# third-party models under va-models/ are missing).
set -euo pipefail
cd "$(dirname "$0")/.."

quick=0
verify_args=()
deny_args=()
case "${1:-}" in
  "") ;;
  --quick) quick=1 ;;
  --cached) verify_args=(--cached); deny_args=(--offline); export CARGO_NET_OFFLINE=true ;;
  *) echo "usage: tools/ci.sh [--quick|--cached]" >&2; exit 2 ;;
esac

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
passed=()
skipped=()

step() {
  local name="$1"; shift
  printf '\n\033[1m==> %s\033[0m\n' "$name"
  local start=$SECONDS
  "$@"
  passed+=("$name ($((SECONDS - start)) s)")
}

skip() {
  printf '\n\033[33m--> skipped %s: %s\033[0m\n' "$1" "$2"
  skipped+=("$1: $2")
}

step "fmt" cargo fmt --all --check
step "clippy" cargo clippy --workspace --all-targets -- -D warnings
step "test" cargo test --workspace

if (( !quick )); then
  step "clippy (all features)" cargo clippy --workspace --all-targets --all-features -- -D warnings
  step "test (all features)" cargo test --workspace --all-features

  msrv=$(sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)
  if rustup run "$msrv" cargo --version >/dev/null 2>&1; then
    step "MSRV ($msrv)" cargo "+$msrv" check --workspace
  else
    skip "MSRV ($msrv)" "run 'rustup toolchain install $msrv'"
  fi

  if command -v cargo-deny >/dev/null 2>&1; then
    step "cargo-deny" cargo deny "${deny_args[@]}" check
  else
    skip "cargo-deny" "run 'cargo install cargo-deny'"
  fi

  if [[ "${1:-}" == --cached ]] || docker image inspect sansho-ngspice:latest >/dev/null 2>&1; then
    step "verify vs ngspice" cargo run -q --release -p sansho-verify --features jit -- "${verify_args[@]}"
    step "verify BSIM3 decks" cargo run -q --release -p sansho-verify -- "${verify_args[@]}" verify/bsim3-decks
    step "verify BSIM4 decks" cargo run -q --release -p sansho-verify -- "${verify_args[@]}" verify/bsim4-decks
    if [[ -d va-models/volare-sky130 ]]; then
      step "verify SKY130 decks" cargo run -q --release -p sansho-verify -- "${verify_args[@]}" verify/sky130-decks
    else
      skip "verify SKY130 decks" "va-models/volare-sky130 is missing"
    fi
    if [[ -d va-models ]]; then
      step "verify Verilog-A decks" cargo run -q --release -p sansho-verify --features jit -- "${verify_args[@]}" verify/va-decks
    else
      skip "verify Verilog-A decks" "va-models/ is missing"
    fi
  else
    skip "verify vs ngspice" "run 'docker build -t sansho-ngspice:latest tools/ngspice'"
  fi
fi

printf '\n\033[1;32mlocal CI passed\033[0m\n'
printf '  ok   %s\n' "${passed[@]}"
if ((${#skipped[@]})); then
  printf '  skip %s\n' "${skipped[@]}"
fi
