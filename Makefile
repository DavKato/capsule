install-dev:
	cargo build --release && cp target/release/capsule ~/.cargo/bin/capsule-dev

uninstall-dev:
	rm -f ~/.cargo/bin/capsule-dev

display-demo:
	cargo run --example display_demo
