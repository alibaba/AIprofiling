.PHONY: all build test clean example

all: build

build:
	cargo build --release

test:
	cargo test

example:
	cargo run --example usage

clean:
	cargo clean

install:
	cargo build --release
	cp target/release/libossrs.so /usr/local/lib/ || true
	cp target/release/libossrs.a /usr/local/lib/ || true

doc:
	cargo doc --no-deps --open
