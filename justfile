# optional file for user for recipes that don't belong
# in version control
import? 'local.justfile'

alias lint := clippy

[private]
@default:
  just --list --justfile {{justfile()}}

# Build the project using cargo
[no-exit-message]
build:
  cargo build

# Test the project using cargo
[no-exit-message]
test:
  cargo nextest run

# Run cargo check on the project
[no-exit-message]
check:
  cargo check

# Run clippy on the project
[no-exit-message]
clippy:
  cargo clippy

# Build documentation using rustdoc
[no-exit-message]
doc:
  cargo doc

setup:
  pre-commit install --install-hooks -t pre-commit -t commit-msg
