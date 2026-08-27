# pesan - common development tasks
.DEFAULT_GOAL := run

# Log level used by the `debug` target (see PESAN_LOG / RUST_LOG).
LOG_LEVEL ?= pesan=debug

.PHONY: run debug install build release test clippy fmt fmt-check check clean help

## run: build and launch the TUI (debug profile)
run:
	cargo run

## debug: launch with verbose logging (LOG_LEVEL=$(LOG_LEVEL), written to the log file)
debug:
	PESAN_LOG=$(LOG_LEVEL) cargo run

## install: build optimized and install the `pesan` binary to ~/.cargo/bin
install:
	cargo install --path . --force

## build: debug build
build:
	cargo build

## release: optimized build (./target/release/pesan)
release:
	cargo build --release

## test: run the test suite
test:
	cargo test

## clippy: lint with clippy, warnings as errors
clippy:
	cargo clippy --all-targets -- -D warnings

## fmt: format the code
fmt:
	cargo fmt

## fmt-check: verify formatting without writing
fmt-check:
	cargo fmt --check

## check: fmt-check + clippy + test (run before committing)
check: fmt-check clippy test

## clean: remove build artifacts
clean:
	cargo clean

## help: list available targets
help:
	@grep -E '^## ' $(MAKEFILE_LIST) | sed 's/^## /  /'
