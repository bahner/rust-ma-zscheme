CRATE = ma-zscheme-ipfs

.PHONY: build check test doc lint fmt fmt-check clean distclean

build:
	cargo build -p $(CRATE)

check:
	cargo check -p $(CRATE)

test: fmt-check
	cargo clippy -p $(CRATE) --all-targets -- -W clippy::pedantic -D warnings
	cargo test -p $(CRATE)

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc -p $(CRATE) --no-deps

lint:
	cargo clippy -p $(CRATE) -- -D warnings

fmt:
	cargo fmt -p $(CRATE)

fmt-check:
	cargo fmt -p $(CRATE) --check

clean:
	@:

distclean: clean
	@:
