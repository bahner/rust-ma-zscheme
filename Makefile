SUBDIRS = ma-zscheme ma-zscheme-yaml ma-zscheme-ipfs

.PHONY: build check test doc lint fmt fmt-check clean distclean

build:
	cargo build --workspace

check:
	cargo check --workspace

test: fmt-check
	cargo clippy --workspace --all-targets -- -W clippy::pedantic -D warnings
	cargo test --workspace

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

lint:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clean:
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir clean; done
	cargo clean

distclean:
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir distclean; done
	cargo clean
	rm -f Cargo.lock
