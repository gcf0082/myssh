# AGENTS.md

## Project Overview

Rust SSH automation tool that connects to remote servers and executes scripted commands via `russh`.

## Commands

- `cargo build` - Build the project
- `cargo run` - Run (requires `config.toml` in project root)
- `cargo test` - Run tests

## Architecture

- **Entry point**: `src/main.rs`
- **Config**: TOML file at project root loaded at runtime (contains SSH credentials - do not commit)
- **Execution flow**: Connect SSH → Run `login_script` steps → Run `command` steps → Stream output until Ctrl+C

## Notes

- `.gitignore` already excludes `config.toml`
- No real tests in `src/lib.rs`; needs real tests added