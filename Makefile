.PHONY: build check test doc lint fmt fmt-check clean distclean

build:
	cargo build

check:
	cargo check
	cargo check --target wasm32-unknown-unknown

test: fmt-check
	cargo clippy --all-targets -- -W clippy::pedantic -D warnings
	cargo test

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

lint:
	cargo clippy -- -D warnings
	mdl README.md

fmt:
	cargo fmt

fmt-check:
	cargo fmt --all --check

clean:
	cargo clean

distclean: clean
	rm -rf Cargo.lock
