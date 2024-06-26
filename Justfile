# Justfile
#
# The command 'just' will give usage information.
# See https://github.com/casey/just for more.

export TOPLEVEL := `pwd`
export CARGO_HOME := TOPLEVEL + "/.cargo"
export GIT_COMMIT_HASH := `git rev-parse --short=8 HEAD`

IMAGE := "mina-indexer:" + GIT_COMMIT_HASH

# Ensure rustfmt works in all environments
# Nix environment has rustfmt nightly and won't work with +nightly
# Non-Nix environment needs nightly toolchain installed and requires +nightly
is_rustfmt_nightly := `cd rust && rustfmt --version | grep stable || echo "true"`
nightly_if_required := if is_rustfmt_nightly == "true" { "" } else { "+nightly" }

default:
  @just --list --justfile {{justfile()}}

# Check for presence of dev dependencies.
tier1-prereqs:
  @echo "--- Checking for tier-1 prereqs"
  ruby --version
  cd rust && cargo --version
  cd rust && cargo nextest --version
  cd rust && cargo audit --version
  cd rust && cargo clippy --version
  cd rust && cargo machete --help 2>&1 >/dev/null
  shellcheck --version

tier2-prereqs: tier1-prereqs
  @echo "--- Checking for tier-2 prereqs"
  jq --version
  check-jsonschema --version
  hurl --version

audit:
  @echo "--- Performing Cargo audit"
  cd rust && time cargo audit

lint:
  @echo "--- Linting ops scripts"
  ruby -cw ops/regression-test
  ruby -cw ops/deploy-local-prod
  ruby -cw ops/granola-rclone
  ruby -cw ops/tier3-test
  ruby -cw ops/download-staking-ledgers
  ruby -cw ops/stage-blocks
  ruby -cw ops/*.rb
  shellcheck tests/regression.bash
  shellcheck ops/deploy
  @echo "--- Linting Rust code"
  cd rust && time cargo {{nightly_if_required}} fmt --all --check
  cd rust && time cargo clippy --all-targets --all-features -- -D warnings
  @echo "--- Linting Nix configs"
  [ "$(nixfmt < flake.nix)" == "$(cat flake.nix)" ]
  @echo "--- Linting Cargo dependencies"
  cd rust && cargo machete Cargo.toml

nix-build:
  @echo "--- Performing Nix build"
  nom build

clean:
  cd rust && cargo clean
  rm -f result
  @echo "Consider also 'git clean -xdfn'"

format:
  cd rust && cargo {{nightly_if_required}} fmt --all

test-unit:
  @echo "--- Performing unit tests"
  cd rust && time cargo nextest run

# Perform a fast verification of whether the source compiles.
check:
  @echo "--- Performing cargo check"
  cd rust && time cargo check

test-unit-mina-rs:
  @echo "--- Performing long-running mina-rs unit tests"
  cd rust && time cargo nextest run --features mina_rs

# Perform a debug build
debug-build:
  cd rust && cargo build

# Quick debug-build and regression-test
bt subtest='': debug-build
  time ./ops/regression-test "$TOPLEVEL"/rust/target/debug/mina-indexer {{subtest}}

# Quick (debug) unit-test and regression-test
tt subtest='': test-unit
  time ./ops/regression-test "$TOPLEVEL"/rust/target/debug/mina-indexer {{subtest}}

# Build OCI images.
build-image:
  @echo "--- Building {{IMAGE}}"
  docker --version
  time nom build .#dockerImage
  time docker load < ./result
  docker run --rm -it {{IMAGE}} mina-indexer server start --help
  docker image rm {{IMAGE}}
  rm result

# Run the 1st tier of tests.
tier1: tier1-prereqs check lint test-unit
  @echo "--- Performing regressions test(s)"
  time ./ops/regression-test "$TOPLEVEL"/rust/target/debug/mina-indexer \
    ipc_is_available_immediately \
    clean_shutdown \
    clean_kill \
    account_balance_cli \
    best_chain \
    rest_accounts_summary \
    hurl

# Run the 2nd tier of tests.
tier2: tier2-prereqs test-unit-mina-rs nix-build && build-image
  @echo "--- Performing regressions test(s) with Nix-built binary"
  time ./ops/regression-test "$TOPLEVEL"/result/bin/mina-indexer
  @echo "--- Performing many_blocks regression test"
  time ./ops/regression-test "$TOPLEVEL"/result/bin/mina-indexer many_blocks
  @echo "--- Performing release"
  time ./ops/regression-test "$TOPLEVEL"/result/bin/mina-indexer release

# Run tier-3 tests.
tier3 magnitude='4': nix-build
  @echo "--- Performing tier3 tests"
  time ./ops/tier3-test {{magnitude}}

# Run a server as if in production.
deploy-local-prod magnitude='4': nix-build
  @echo "--- Deploying to production"
  time ./ops/deploy-local-prod {{magnitude}}
