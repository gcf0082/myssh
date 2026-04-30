#!/usr/bin/env python3
"""myssh (Python) — parallel SSH command runner.

Functional parity with the Rust version (single-shot mode):
  --command / -c, --nodes / -n, --ip, --prefix, --sync, --list-nodes,
  --verbose / -v.

Reuses the same config.yaml format and the same remote command protocol
(echo MY_begin; echo <b64> | base64 -d | bash -s; echo MY_end). Two
implementations are interchangeable on the same fleet.

Not in v1: --interactive (-i) and the !! meta-commands.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import os
import re
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

try:
    import yaml
    import asyncssh
except ImportError as e:  # pragma: no cover
    sys.stderr.write(
        f"Missing dependency: {e.name}. "
        f"Run: pip install -r requirements.txt\n"
    )
    sys.exit(1)


# =============================================================================
# Constants (mirror Rust version)
# =============================================================================

DEFAULT_TERM_COLS = 200
DEFAULT_TERM_ROWS = 60
LOGIN_STEP_TIMEOUT = 30        # seconds
COMMAND_PROMPT_TIMEOUT = 30    # seconds


# =============================================================================
# Config loading
# =============================================================================

def home_dir() -> Optional[Path]:
    """Cross-platform user home directory."""
    if sys.platform == "win32":
        v = os.environ.get("USERPROFILE")
    else:
        v = os.environ.get("HOME")
    return Path(v) if v else None


def exe_dir() -> Optional[Path]:
    """Directory containing this myssh.py script."""
    try:
        return Path(__file__).resolve().parent
    except NameError:
        return None


def resolve_config_paths() -> list[Path]:
    """Lookup order: <exe-dir>/config.yaml, ~/.myssh/config.yaml."""
    paths: list[Path] = []
    e = exe_dir()
    if e:
        paths.append(e / "config.yaml")
    h = home_dir()
    if h:
        paths.append(h / ".myssh" / "config.yaml")
    return paths


@dataclass
class ScriptStep:
    name: str
    wait: str
    send: str


@dataclass
class NodeCfg:
    id: str
    host: str
    port: int = 22
    user: str = ""
    password: str = ""
    login_script: list[ScriptStep] = field(default_factory=list)
    login_script_append: list[ScriptStep] = field(default_factory=list)
    use_jump: Optional[bool] = None


@dataclass
class JumpCfg:
    host: str = ""
    port: int = 22
    user: str = ""
    password: str = ""
    login_script: list[ScriptStep] = field(default_factory=list)


@dataclass
class DefaultsCfg:
    port: int = 22
    user: str = ""
    password: str = ""
    login_script: list[ScriptStep] = field(default_factory=list)
    use_jump: bool = False
    command_wait: str = "$|#"


@dataclass
class Config:
    defaults: DefaultsCfg
    jump: JumpCfg
    nodes: list[NodeCfg]


def _parse_steps(raw) -> list[ScriptStep]:
    out: list[ScriptStep] = []
    for s in (raw or []):
        if not isinstance(s, dict):
            continue
        out.append(ScriptStep(
            name=str(s.get("name", "")),
            wait=str(s.get("wait", "")),
            send=str(s.get("send", "")),
        ))
    return out


def parse_config(raw: dict) -> Config:
    d = raw.get("defaults") or {}
    defaults = DefaultsCfg(
        port=int(d.get("port", 22)),
        user=str(d.get("user", "")),
        password=str(d.get("password", "")),
        login_script=_parse_steps(d.get("login_script")),
        use_jump=bool(d.get("use_jump", False)),
        command_wait=str(d.get("command_wait", "$|#")),
    )
    j = raw.get("jump") or {}
    jump = JumpCfg(
        host=str(j.get("host", "")),
        port=int(j.get("port", 22)),
        user=str(j.get("user", "")),
        password=str(j.get("password", "")),
        login_script=_parse_steps(j.get("login_script")),
    )
    nodes_raw = raw.get("nodes") or []
    nodes: list[NodeCfg] = []
    for n in nodes_raw:
        if not isinstance(n, dict) or "id" not in n or "host" not in n:
            sys.exit(f"Error: node missing required field id or host: {n}")
        nodes.append(NodeCfg(
            id=str(n["id"]),
            host=str(n["host"]),
            port=int(n.get("port", 22)),
            user=str(n.get("user", "")),
            password=str(n.get("password", "")),
            login_script=_parse_steps(n.get("login_script")),
            login_script_append=_parse_steps(n.get("login_script_append")),
            use_jump=n.get("use_jump"),
        ))
    return Config(defaults=defaults, jump=jump, nodes=nodes)


def load_config() -> Config:
    paths = resolve_config_paths()
    for p in paths:
        if p.is_file():
            with open(p, "r", encoding="utf-8") as f:
                raw = yaml.safe_load(f) or {}
            return parse_config(raw)
    tried = ", ".join(str(p) for p in paths) if paths else "<no candidates>"
    sys.exit(f"Error: config.yaml not found. Tried (in order): {tried}")


# =============================================================================
# Helpers
# =============================================================================

# ANSI escape sequences. Strip during prompt matching only — the user's command
# output goes through line_buffer untouched.
_ANSI_OSC = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
_ANSI_CSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
_ANSI_OTHER = re.compile(r"\x1b[@-Z\\-_]")


def strip_ansi(s: str) -> str:
    s = _ANSI_OSC.sub("", s)
    s = _ANSI_CSI.sub("", s)
    s = _ANSI_OTHER.sub("", s)
    return s


def check_wait(pattern: str, output: str) -> bool:
    """True if any of the |-separated patterns appears as a substring in output."""
    for p in pattern.split("|"):
        p = p.strip()
        if p and p in output:
            return True
    return False


def get_term_size() -> tuple[int, int]:
    try:
        size = shutil.get_terminal_size((DEFAULT_TERM_COLS, DEFAULT_TERM_ROWS))
        return size.columns, size.lines
    except Exception:
        return DEFAULT_TERM_COLS, DEFAULT_TERM_ROWS


def build_login_script(
    defaults: DefaultsCfg, node: NodeCfg, password: str
) -> list[ScriptStep]:
    """Mirror Rust's build_login_script: node.login_script overrides defaults
    entirely; otherwise defaults.login_script + node.login_script_append.
    {{password}} placeholders are substituted with the node's resolved password.
    """
    def subst(steps: list[ScriptStep]) -> list[ScriptStep]:
        return [
            ScriptStep(s.name, s.wait, password if s.send == "{{password}}" else s.send)
            for s in steps
        ]

    if node.login_script:
        return subst(node.login_script)
    return subst(defaults.login_script) + list(node.login_script_append)


# =============================================================================
# Node listing & target resolution
# =============================================================================

def format_node_list(
    cfg: Config, verbose: bool, filter_ids: Optional[set[str]]
) -> list[str]:
    selected = [n for n in cfg.nodes if filter_ids is None or n.id in filter_ids]
    if not verbose:
        return [", ".join(n.id for n in selected)]

    out: list[str] = []
    default_port = cfg.defaults.port
    for n in selected:
        user = n.user or cfg.defaults.user
        port = default_port if (n.port == 22 and default_port != 22) else n.port
        use_jump = n.use_jump if n.use_jump is not None else cfg.defaults.use_jump
        jump = "via-jump" if use_jump else "direct"
        out.append(f"{n.id}\t{n.host}:{port}\t{user}\t{jump}")
    return out


def resolve_targets(
    args: argparse.Namespace, cfg: Config
) -> Optional[set[str]]:
    """--ip <addr> looks up by host; --nodes <list> validates ids; otherwise None (=all)."""
    if args.ip:
        matches = [n for n in cfg.nodes if n.host == args.ip]
        if not matches:
            sys.exit(f"Error: No node found in config.yaml with host: {args.ip}")
        if len(matches) > 1:
            ids = ", ".join(n.id for n in matches)
            sys.exit(
                f"Error: Multiple nodes share host {args.ip}: {ids}. "
                f"Use --nodes <id> to disambiguate."
            )
        return {matches[0].id}

    if args.nodes:
        requested = {s.strip() for s in args.nodes.split(",") if s.strip()}
        all_ids = {n.id for n in cfg.nodes}
        missing = sorted(requested - all_ids)
        if missing:
            sys.exit(f"Error: Node(s) not found: {', '.join(missing)}")
        return requested

    return None


# =============================================================================
# LineBuffer — output formatter (parity with Rust's LineBuffer)
# =============================================================================

class LineBuffer:
    """Either streams lines to stdout (with optional [node] prefix) or captures
    them into an internal list (--sync mode)."""

    def __init__(self, node_id: str, capture: bool, lock: asyncio.Lock):
        self._prefix = f"[{node_id}]"
        self._buf = ""
        self._captured: Optional[list[str]] = [] if capture else None
        self._lock = lock

    async def _emit(self, line: str, with_prefix: bool) -> None:
        formatted = f"{self._prefix} {line}" if with_prefix else line
        if self._captured is not None:
            self._captured.append(formatted)
            return
        async with self._lock:
            sys.stdout.write(formatted + "\n")
            sys.stdout.flush()

    async def feed(self, data: str, with_prefix: bool) -> None:
        self._buf += data
        while "\n" in self._buf:
            idx = self._buf.index("\n")
            line = self._buf[:idx]
            self._buf = self._buf[idx + 1:]
            await self._emit(line, with_prefix)

    async def flush(self, with_prefix: bool) -> None:
        if self._buf:
            line = self._buf
            self._buf = ""
            await self._emit(line, with_prefix)

    def drain(self) -> list[str]:
        out = self._captured or []
        self._captured = None
        return out


# =============================================================================
# Core SSH flow
# =============================================================================

async def _read_until(
    proc: "asyncssh.SSHClientProcess",
    pattern: str,
    timeout: float,
    verbose: bool,
    who: str,
) -> str:
    """Read from stdout until any |-separated `pattern` appears (after ANSI
    strip) or `timeout` elapses or EOF. Returns accumulated raw buffer."""
    buf = ""
    loop = asyncio.get_event_loop()
    deadline = loop.time() + timeout
    while True:
        remaining = deadline - loop.time()
        if remaining <= 0:
            raise asyncio.TimeoutError()
        try:
            chunk = await asyncio.wait_for(proc.stdout.read(8192), timeout=remaining)
        except asyncio.TimeoutError:
            raise
        if not chunk:
            raise EOFError("channel closed")
        if isinstance(chunk, bytes):
            chunk = chunk.decode(errors="replace")
        buf += chunk
        if verbose:
            sys.stderr.write(f"[DEBUG][{who}] Received: {chunk!r}\n")
            sys.stderr.flush()
        if check_wait(pattern, strip_ansi(buf)):
            return buf


async def run_login_script(
    proc: "asyncssh.SSHClientProcess",
    steps: list[ScriptStep],
    verbose: bool,
    who: str,
) -> None:
    for step in steps:
        if verbose:
            sys.stderr.write(
                f"[DEBUG][{who}] Login step: {step.name} - send: {step.send}\n"
            )
            sys.stderr.flush()
        try:
            await _read_until(proc, step.wait, LOGIN_STEP_TIMEOUT, verbose, who)
        except asyncio.TimeoutError:
            raise RuntimeError(
                f"{who} - Login script step '{step.name}' failed: "
                f"Timeout waiting for pattern '{step.wait}'"
            )
        except EOFError:
            raise RuntimeError(
                f"{who} - Login script step '{step.name}' failed: Channel closed"
            )
        proc.stdin.write(step.send + "\n")
        await proc.stdin.drain()


async def run_command(
    proc: "asyncssh.SSHClientProcess",
    command: str,
    command_wait: str,
    prefix: bool,
    line_buf: LineBuffer,
    verbose: bool,
    who: str,
) -> bool:
    """Send the user command via the MY_begin/MY_end protocol and stream output
    through line_buf. Returns True on clean MY_end, False on EOF mid-stream."""
    # 1. wait for prompt
    try:
        await _read_until(proc, command_wait, COMMAND_PROMPT_TIMEOUT, verbose, who)
    except asyncio.TimeoutError:
        raise RuntimeError(
            f"{who} - Command execution failed: Timeout waiting for prompt"
        )
    except EOFError:
        raise RuntimeError(
            f"{who} - Command execution failed: Channel closed"
        )

    # 2. send wrapped, base64-isolated command
    encoded = base64.b64encode(command.encode("utf-8")).decode("ascii")
    wrapped = f"echo MY_begin;echo {encoded} | base64 -d | bash -s;echo MY_end\n"
    proc.stdin.write(wrapped)
    await proc.stdin.drain()

    # 3. stream output: skip noise until MY_begin\r\n, then forward bytes to
    #    line_buf until MY_end. No timeout here — supports tail -f / streaming.
    full = ""
    output_start = 0
    while True:
        try:
            chunk = await proc.stdout.read(8192)
        except asyncssh.misc.ConnectionLost:
            break
        if not chunk:
            break
        if isinstance(chunk, bytes):
            chunk = chunk.decode(errors="replace")
        full += chunk
        begin_pos = full.find("MY_begin\r\n")
        if begin_pos < 0:
            continue
        content_start = begin_pos + len("MY_begin\r\n")
        if content_start > output_start:
            output_start = content_start
        content = full[output_start:]
        end_pos = content.find("MY_end")
        if end_pos >= 0:
            await line_buf.feed(content[:end_pos], prefix)
            await line_buf.flush(prefix)
            return True
        await line_buf.feed(content, prefix)
        output_start = len(full)

    await line_buf.flush(prefix)
    return False


def _connect_kwargs(host: str, port: int, user: str, password: str) -> dict:
    """Common asyncssh.connect args. known_hosts=None mirrors Rust's
    check_server_key → Ok(true) (accept any host key)."""
    return dict(
        host=host,
        port=port,
        username=user,
        password=password,
        known_hosts=None,
        client_keys=None,        # don't try ssh-agent / ~/.ssh keys
        preferred_auth=("password", "keyboard-interactive"),
    )


async def execute_node(
    node: NodeCfg,
    defaults: DefaultsCfg,
    jump: JumpCfg,
    command: str,
    verbose: bool,
    prefix: bool,
    capture: bool,
    sync_lock: asyncio.Lock,
) -> tuple[bool, list[str]]:
    """Connect (direct or via jump), run login_script, run the command.
    Returns (success, captured_lines). Raises on connection / auth / protocol
    failure — caller logs and counts as failed."""
    user = node.user or defaults.user
    password = node.password or defaults.password
    port = defaults.port if (node.port == 22 and defaults.port != 22) else node.port
    use_jump = node.use_jump if node.use_jump is not None else defaults.use_jump
    login_script = build_login_script(defaults, node, password)
    command_wait = defaults.command_wait
    cols, rows = get_term_size()

    line_buf = LineBuffer(node.id, capture, sync_lock)

    if verbose:
        sys.stderr.write(
            f"[DEBUG][{node.id}] Connecting to {node.host}:{port} as {user}"
            f"{' via jump' if use_jump else ''}\n"
        )
        sys.stderr.flush()

    try:
        if use_jump:
            jump_user = jump.user or defaults.user
            jump_password = jump.password or defaults.password
            async with asyncssh.connect(
                **_connect_kwargs(jump.host, jump.port, jump_user, jump_password)
            ) as jump_conn:
                if jump.login_script:
                    async with jump_conn.create_process(
                        term_type="xterm", term_size=(cols, rows),
                        encoding="utf-8", errors="replace",
                    ) as jump_proc:
                        await run_login_script(
                            jump_proc, jump.login_script, verbose, f"{node.id}/jump"
                        )
                async with asyncssh.connect(
                    **_connect_kwargs(node.host, port, user, password),
                    tunnel=jump_conn,
                ) as conn:
                    async with conn.create_process(
                        term_type="xterm", term_size=(cols, rows),
                        encoding="utf-8", errors="replace",
                    ) as proc:
                        await run_login_script(proc, login_script, verbose, node.id)
                        ok = await run_command(
                            proc, command, command_wait, prefix, line_buf, verbose, node.id
                        )
        else:
            async with asyncssh.connect(
                **_connect_kwargs(node.host, port, user, password)
            ) as conn:
                async with conn.create_process(
                    term_type="xterm", term_size=(cols, rows),
                    encoding="utf-8", errors="replace",
                ) as proc:
                    await run_login_script(proc, login_script, verbose, node.id)
                    ok = await run_command(
                        proc, command, command_wait, prefix, line_buf, verbose, node.id
                    )
    except asyncssh.PermissionDenied:
        await line_buf.flush(prefix)
        raise RuntimeError(
            f"{node.host}:{port}:{user} - Authentication failed: Invalid password or username"
        )
    except (OSError, asyncssh.Error) as e:
        await line_buf.flush(prefix)
        raise RuntimeError(f"{node.host}:{port}:{user} - {type(e).__name__}: {e}")

    return ok, line_buf.drain()


# =============================================================================
# Orchestration
# =============================================================================

async def execute_on_all_nodes(
    args: argparse.Namespace,
    cfg: Config,
    command: str,
    target_ids: Optional[set[str]],
) -> bool:
    """Spawn parallel tasks for each target node; return True if any failed."""
    nodes = [n for n in cfg.nodes if target_ids is None or n.id in target_ids]
    if not nodes:
        return False

    capture = args.sync
    sync_lock = asyncio.Lock()

    async def run_one(node: NodeCfg) -> tuple[bool, list[str]]:
        try:
            return await execute_node(
                node, cfg.defaults, cfg.jump, command,
                args.verbose, args.prefix, capture, sync_lock,
            )
        except Exception as e:
            sys.stderr.write(f"\nTask failed: {e}\n")
            sys.stderr.flush()
            return False, []

    # Spawn order = nodes-list order = config / --nodes order. Awaiting in the
    # same order means --sync prints blocks deterministically by config order.
    tasks = [asyncio.create_task(run_one(n)) for n in nodes]

    try:
        results = await asyncio.gather(*tasks, return_exceptions=False)
    except (KeyboardInterrupt, asyncio.CancelledError):
        sys.stderr.write("\nCtrl+C received, aborting all tasks...\n")
        for t in tasks:
            t.cancel()
        return True

    any_failed = False
    for ok, lines in results:
        for line in lines:
            sys.stdout.write(line + "\n")
        sys.stdout.flush()
        if not ok:
            any_failed = True

    return any_failed


# =============================================================================
# CLI
# =============================================================================

EXAMPLES = """\
Examples:
  myssh --command 'cat /etc/hostname'                         Run on all nodes
  myssh --command 'cat /etc/hostname' --nodes node1,node3     Run only on the given nodes
  myssh --command 'cat /etc/hostname' --ip 1.2.3.4            Pick a configured node by host/IP
  myssh --command 'cat /etc/hostname' --prefix                Prefix each output line with [node]
  myssh --command 'cat /etc/hostname' --sync                  Parallel exec, grouped per-node output
                                                              (do NOT use with tail -f / streaming commands)
  myssh --list-nodes                                          List node IDs from config.yaml
  myssh --list-nodes --verbose                                List node details (id / host:port / user / jump)
"""


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="myssh",
        description="Parallel SSH command runner (Python port of myssh).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=EXAMPLES,
    )
    p.add_argument("-c", "--command",
                   help="Command to execute on remote nodes")
    p.add_argument("-v", "--verbose", action="store_true",
                   help="Show debug information on stderr")
    p.add_argument("--prefix", action="store_true",
                   help="Add node prefix to each output line")
    p.add_argument("--sync", action="store_true",
                   help="Parallel execute but print each node's output as one grouped block in node order")
    p.add_argument("--list-nodes", dest="list_nodes", action="store_true",
                   help="List nodes from config.yaml and exit (honors -v for details and -n to filter)")
    # --interactive is a recognized flag so we can give a clear v1-not-supported message.
    p.add_argument("-i", "--interactive", action="store_true",
                   help=argparse.SUPPRESS)

    # --nodes / --ip are mutually exclusive
    target = p.add_mutually_exclusive_group()
    target.add_argument("-n", "--nodes",
                        help="Comma-separated list of node IDs to execute on")
    target.add_argument("--ip", metavar="ADDR",
                        help="Select a configured node by its host/IP instead of by id")

    args = p.parse_args()

    if args.interactive:
        sys.exit(
            "Error: --interactive is not implemented in the Python port. "
            "Use the Rust myssh binary, or pass --command explicitly."
        )

    return args


def main() -> None:
    args = parse_args()
    cfg = load_config()

    if args.list_nodes:
        target_ids = resolve_targets(args, cfg)
        for line in format_node_list(cfg, args.verbose, target_ids):
            print(line)
        return

    if not args.command:
        sys.exit("Error: --command (-c) is required (or use --list-nodes).")

    target_ids = resolve_targets(args, cfg)

    try:
        any_failed = asyncio.run(
            execute_on_all_nodes(args, cfg, args.command, target_ids)
        )
    except KeyboardInterrupt:
        sys.stderr.write("\nCtrl+C received, aborting...\n")
        sys.exit(1)

    if any_failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
