[doc('Build the zb binary')]
[group('build')]
build: fmt-check lint
    cargo build --release --bin zb --bin zbx

