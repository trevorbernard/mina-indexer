# Justfile
#
# The command 'just' will give usage information.
# See https://github.com/casey/just for more.

# Ensure rustfmt works in all environments
# Nix environment has rustfmt nightly and won't work with +nightly
# Non-Nix environment needs nightly toolchain installed and requires +nightly
is_rustfmt_nightly := `cd rust && rustfmt --version | grep stable || echo "true"`
nightly_if_required := if is_rustfmt_nightly == "true" { "" } else { "+nightly" }

default:
  @just --list --justfile {{justfile()}}

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
  rm -rf result

format:
  cd rust && cargo {{nightly_if_required}} fmt --all

test: lint test-unit test-regression

test-unit:
  cd rust && cargo nextest run --release

test-unit-mina-rs:
  cd rust && cargo nextest run --release --features mina_rs

test-regression: build
  ./tests/regression

test-release: build
  ./tests/regression test_release

disallow-unused-cargo-deps:
  cd rust && cargo machete Cargo.toml

audit:
  cd rust && cargo audit

lint: && audit disallow-unused-cargo-deps
  shellcheck tests/regression
  shellcheck tests/stage-*
  cd rust && cargo {{nightly_if_required}} fmt --all --check
  cd rust && cargo clippy --all-targets --all-features -- -D warnings
  [ "$(nixfmt < flake.nix)" == "$(cat flake.nix)" ]

images:
  docker --version
  nix build .#dockerImage
  docker load < ./result
