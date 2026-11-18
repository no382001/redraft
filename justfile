build:
    cargo build --release

run repo="." *ARGS:
    cargo run -- "{{invocation_directory()}}/{{repo}}" {{ARGS}}

test:
    cargo test
    cargo clippy --all-targets

install:
    cargo install --path .
