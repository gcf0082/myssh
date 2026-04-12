# AGENTS.md

## Project Overview

Rust SSH automation tool that connects to multiple remote servers in parallel and executes scripted commands via `russh`.

## Commands

- `cargo build` - Build the project
- `cargo run -- --command <cmd>` - Run on all nodes (requires `config.yaml` in project root)
- `cargo run -- --command <cmd> --nodes <id1,id2>` - Run on specific nodes only
- `cargo test` - Run tests

## Architecture

- **Entry point**: `src/main.rs`
- **Config**: YAML file at project root loaded at runtime (contains SSH credentials - do not commit)
- **Execution flow**: Parse config → Filter nodes by --nodes argument → Spawn parallel SSH connections for each node → Run `login_script` steps → Execute CLI command → Stream output until Ctrl+C

## Configuration File Format

**config.yaml.example**:
```yaml
nodes:
  - id: "node1"
    host: "1.1.1.1"
    port: 22
    user: "user"
    password: "pass"
  - id: "node2"
    host: "1.1.1.2"
    port: 22
    user: "user"
    password: "pass"

login_script:
  - name: "SSH登录"
    wait: "$"
    send: "su - root"
  - name: "输入密码"
    wait: "Password:"
    send: "pass"
```

## Notes

- `.gitignore` already excludes `config.yaml`
- Commands must be specified via CLI argument `--command` or `-c`
- Use `--nodes` or `-n` to filter which nodes to execute on (comma-separated list)
- If `--nodes` is not specified, commands will run on all nodes
- Multiple nodes execute commands in parallel
- No real tests in `src/lib.rs`; needs real tests added

## Examples

```bash
# Execute command on all nodes
cargo run -- --command "ls -la"

# Execute command on specific nodes
cargo run -- --command "hostname" --nodes node1,node3

# Using short options
cargo run -- -c "pwd" -n node2
```
