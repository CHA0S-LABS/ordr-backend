.PHONY: build run dev test lint check clean help

build:
	cargo build

build-release:
	cargo build --release

run:
	cargo run

dev:
	cargo watch -x run

test:
	cargo test

test-verbose:
	cargo test -- --nocapture

format:
	cargo +nightly fmt --all

fmt-check:
	cargo fmt --check

lint:
	cargo clippy -- -D warnings

check:
	cargo check

db-migrate:
	sqlx migrate run

db-revert:
	sqlx migrate revert

db-prepare:
	cargo sqlx prepare

clean:
	cargo clean

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  build          Build (debug)"
	@echo "  build-release  Build (release)"
	@echo "  run            Run the backend"
	@echo "  dev            Run with auto-reload (requires cargo-watch)"
	@echo "  test           Run tests"
	@echo "  test-verbose   Run tests with stdout"
	@echo "  format         Format code"
	@echo "  fmt-check      Check formatting without modifying"
	@echo "  lint           Run clippy (warnings = errors)"
	@echo "  check          Type-check without building"
	@echo "  db-migrate     Run pending migrations"
	@echo "  db-revert      Revert last migration"
	@echo "  db-prepare     Generate sqlx query metadata"
	@echo "  clean          Remove build artifacts"
