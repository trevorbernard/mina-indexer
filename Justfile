# Justfile
#
# The command 'just' will give usage information.
# See https://github.com/casey/just for more.

export GIT_COMMIT_HASH := `git rev-parse --short=8 HEAD`
export CARGO_HOME := `pwd` + ".cargo"

IMAGE := "mina-indexer:" + GIT_COMMIT_HASH

# Ensure rustfmt works in all environments
# Nix environment has rustfmt nightly and won't work with +nightly
# Non-Nix environment needs nightly toolchain installed and requires +nightly
is_rustfmt_nightly := `cd rust && rustfmt --version | grep stable || echo "true"`
nightly_if_required := if is_rustfmt_nightly == "true" { "" } else { "+nightly" }

default:
  @just --list --justfile {{justfile()}}

# Check for presence of dev dependencies.
prereqs:
  cd rust && cargo --version
  cd rust && cargo nextest --version
  cd rust && cargo audit --version
  cd rust && cargo clippy --version
  cd rust && cargo machete --help 2>&1 >/dev/null
  jq --version
  check-jsonschema --version
  hurl --version
  shellcheck --version

build:
  cd rust && cargo build --release

clean:
  cd rust && cargo clean
  rm -rf result database mina-indexer.sock

format:
  cd rust && cargo {{nightly_if_required}} fmt --all

test: lint test-unit test-regression

test-unit:
  cd rust && cargo nextest run --release

test-unit-mina-rs:
  cd rust && cargo nextest run --release --features mina_rs

test-regression subtest='': build
  ./tests/regression {{subtest}}

test-release: build
  ./tests/regression test_release

disallow-unused-cargo-deps:
  cd rust && cargo machete Cargo.toml

audit:
  cd rust && cargo audit

lint: && audit disallow-unused-cargo-deps
  shellcheck tests/regression
  shellcheck tests/stage-*
  shellcheck ops/productionize
  shellcheck ops/ingest-all
  cd rust && cargo {{nightly_if_required}} fmt --all --check
  cd rust && cargo clippy --all-targets --all-features -- -D warnings
  [ "$(nixfmt < flake.nix)" == "$(cat flake.nix)" ]

# Build OCI images.
build-image:
  echo "Building {{IMAGE}}"
  docker --version
  nix build .#dockerImage
  docker load < ./result
  docker run --rm -it {{IMAGE}} \
    mina-indexer server start --help
  docker image rm {{IMAGE}}

# Start a server in the current directory.
start-server: build
  RUST_BACKTRACE=1 \
  ./rust/target/release/mina-indexer \
    --domain-socket-path ./mina-indexer.sock \
    server start \
      --log-level TRACE \
      --blocks-dir ./tests/data/initial-blocks \
      --staking-ledgers-dir ./tests/data/staking_ledgers \
      --database-dir ./database

# Delete the database created by 'start-server'.
delete-database:
  rm -fr ./database

# Run a server as if in production.
productionize: build
  ./ops/productionize

# Run the 1st tier of tests.
tier1-test: prereqs test

# Run the 2nd tier of tests, ingesting blocks in /mnt/mina-logs...
tier2-test: build
  tests/regression test_many_blocks
  nix build
  ops/ingest-all
