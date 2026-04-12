# AGENTS.md

## Build & Run

```bash
cargo build --release
./target/release/myssh
```

## Test

```bash
./test.sh
```

Or: `cargo test`

## Requirements

- `config.toml` must exist in working directory (reads from CWD, not relative to binary)
- Real SSH server needed for full integration testing