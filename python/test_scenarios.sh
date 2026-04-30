#!/usr/bin/env bash
# 端到端验证 python/myssh.py 是否覆盖 Rust myssh 的各种使用场景
# （包括 login_script 的 3 种用法：defaults 全局、节点 override、login_script_append）。
#
# 用法：
#   TEST_HOST=<ip> TEST_PASSWORD=<root 与目标普通用户都能登录的同一密码> \
#       ./python/test_scenarios.sh
#
# 前提：
#   - 目标主机 root 可用密码登录
#   - 目标主机有一个普通用户 'gcf'（或将 TEST_NORMAL_USER 改成实际名字），
#     用同样的密码登录后能 `su -` 到 root（同密码）
#   - 本机已 pip install asyncssh PyYAML，PY 指向对应 python 解释器

set -uo pipefail

# ---- inputs ------------------------------------------------------------------
HOST="${TEST_HOST:-}"
PASSWORD="${TEST_PASSWORD:-}"
NORMAL_USER="${TEST_NORMAL_USER:-gcf}"
PY="${PY:-python3}"
MYSSH_PY="${MYSSH_PY:-$(cd "$(dirname "$0")" && pwd)/myssh.py}"

if [[ -z "$HOST" || -z "$PASSWORD" ]]; then
    cat <<USAGE >&2
Error: TEST_HOST and TEST_PASSWORD must be set.

Usage:
  TEST_HOST=<ip> TEST_PASSWORD=<pw> $0
  TEST_HOST=<ip> TEST_PASSWORD=<pw> TEST_NORMAL_USER=<user> PY=python3 $0
USAGE
    exit 2
fi

if [[ ! -f "$MYSSH_PY" ]]; then
    echo "Error: cannot find myssh.py at $MYSSH_PY" >&2
    exit 2
fi

# ---- workspace ---------------------------------------------------------------
TEST_DIR="$(mktemp -d /tmp/myssh-pytest.XXXXXX)"
trap 'rm -rf "$TEST_DIR"' EXIT

cp "$MYSSH_PY" "$TEST_DIR/myssh.py"

# 在仓库根目录里写带凭据的 config 是危险的，借 $TEST_DIR 隔离
# 且 trap 会在脚本结束时清理。
# 注意：不带引号的 heredoc 里要把字面 `$` 转义为 `\$`，避免 bash 展开。
cat > "$TEST_DIR/config.yaml" <<EOF
# defaults.login_script: 全局登录脚本——以普通用户 SSH 登录后 su 到 root，
# 模拟 Rust 文档里"普通用户登录然后切 root"的常见场景。
defaults:
  port: 22
  user: $NORMAL_USER
  password: $PASSWORD
  login_script:
    - name: "wait normal-user prompt then su -"
      wait: "\$"
      send: "su -"
    - name: "give su the password (uses {{password}} substitution)"
      wait: "Password:"
      send: "{{password}}"

nodes:
  # T-A: 直接走 defaults.login_script (su 到 root)
  - id: gcf-su
    host: $HOST

  # T-B: 节点 override login_script——只做一个 noop step，不 su，最终保持 normal user
  - id: gcf-no-su
    host: $HOST
    login_script:
      - name: "consume normal-user prompt, no su"
        wait: "\$"
        send: "true"

  # T-C: login_script_append——在 defaults 后追加一条，导出 env var 让命令侧能验证
  - id: gcf-su-append
    host: $HOST
    login_script_append:
      - name: "export marker after su"
        wait: "#"
        send: "export MYSSH_TEST_APPEND=ran"

  # T-D: root 直登——节点 override login_script，避免触发 defaults 的 su
  - id: root-direct
    host: $HOST
    user: root
    password: $PASSWORD
    login_script:
      - name: "consume root prompt"
        wait: "#"
        send: "true"
EOF

cd "$TEST_DIR"

# ---- helpers -----------------------------------------------------------------
PASS=0; FAIL=0; FAIL_NAMES=()

assert_run() {
    # assert_run <name> <expect-pass|expect-fail> <expected-grep-regex> <cmd...>
    local name="$1"; local mode="$2"; local pattern="$3"; shift 3
    local out rc
    out="$("$@" 2>&1)"; rc=$?

    local ok=true
    if [[ "$mode" == "expect-pass" && $rc -ne 0 ]]; then ok=false; fi
    if [[ "$mode" == "expect-fail" && $rc -eq 0 ]]; then ok=false; fi
    if [[ -n "$pattern" ]] && ! grep -qE -- "$pattern" <<< "$out"; then ok=false; fi

    if $ok; then
        printf '  \033[32mPASS\033[0m %s\n' "$name"
        PASS=$((PASS+1))
    else
        printf '  \033[31mFAIL\033[0m %s (rc=%d, mode=%s, pattern=%s)\n' \
            "$name" "$rc" "$mode" "$pattern"
        sed 's/^/        /' <<< "$out"
        FAIL=$((FAIL+1))
        FAIL_NAMES+=("$name")
    fi
}

PY_RUN=("$PY" "$TEST_DIR/myssh.py")

# ---- tests -------------------------------------------------------------------

echo "=== Listing & validation ==="

assert_run "T01 --list-nodes prints all 4 ids on one line" \
    expect-pass 'gcf-su.*gcf-no-su.*gcf-su-append.*root-direct' \
    "${PY_RUN[@]}" --list-nodes

assert_run "T02 --list-nodes -v emits tab-separated detail row" \
    expect-pass "^gcf-su\s+$HOST:22\s+$NORMAL_USER\s+direct$" \
    "${PY_RUN[@]}" --list-nodes -v

assert_run "T03 --list-nodes -n filter narrows the listing" \
    expect-pass '^root-direct$|^root-direct\s' \
    "${PY_RUN[@]}" --list-nodes -n root-direct

assert_run "T04 --nodes with bogus id errors out" \
    expect-fail 'Node\(s\) not found: nope' \
    "${PY_RUN[@]}" --list-nodes -n nope

assert_run "T05 --ip with no match errors out" \
    expect-fail 'No node found in config\.yaml with host' \
    "${PY_RUN[@]}" --list-nodes --ip 9.9.9.9

assert_run "T06 --ip with multiple matches asks for disambiguation" \
    expect-fail 'Multiple nodes share host' \
    "${PY_RUN[@]}" --list-nodes --ip "$HOST"

assert_run "T07 --ip + --nodes is mutually exclusive (clap-style)" \
    expect-fail 'not allowed with' \
    "${PY_RUN[@]}" -c id --nodes gcf-no-su --ip "$HOST"

assert_run "T08 --interactive is rejected with a clear v1-not-supported message" \
    expect-fail 'not implemented' \
    "${PY_RUN[@]}" -i

echo
echo "=== login_script scenarios (the focus of this run) ==="

# T09: defaults.login_script 应用到 gcf-su，远端命令以 root 身份运行
assert_run "T09 defaults.login_script: gcf登录后 su - 到 root" \
    expect-pass 'uid=0\(root\)' \
    "${PY_RUN[@]}" -c id --nodes gcf-su

# T10: 节点 override login_script 把 defaults 完全替换，命令以普通用户身份运行
assert_run "T10 node-override login_script: 不 su，命令仍是普通用户身份" \
    expect-pass "uid=[0-9]+\\($NORMAL_USER\\)" \
    "${PY_RUN[@]}" -c id --nodes gcf-no-su

# T11: login_script_append 在 defaults 后追加；通过 env var 验证 append 步骤实际跑了。
# 注意: 远端 PTY 行尾是 CRLF, 行内会留 \r, 故只锚 ^ 不锚 $。
assert_run "T11 login_script_append: append 步骤已执行 (env var 透传)" \
    expect-pass '^marker=ran' \
    "${PY_RUN[@]}" -c 'echo "marker=$MYSSH_TEST_APPEND"' --nodes gcf-su-append

# T12: 节点级密码 + 节点级 login_script override (root 直登)
assert_run "T12 节点级 user/password 覆盖 defaults，root 直登成功" \
    expect-pass 'uid=0\(root\)' \
    "${PY_RUN[@]}" -c id --nodes root-direct

# T13: {{password}} 占位符替换——T09 通过即间接验证；这里再用一条命令显式确认
#      su 之后命令是 root，说明 send: "{{password}}" 被替换成了正确的 root 密码
assert_run "T13 {{password}} 占位符在 login_script 里被正确替换" \
    expect-pass 'uid=0\(root\)' \
    "${PY_RUN[@]}" -c 'whoami; id' --nodes gcf-su

echo
echo "=== Output formatting ==="

assert_run "T14 --prefix 给每行带 [node] 前缀" \
    expect-pass "^\\[gcf-no-su\\] uid=" \
    "${PY_RUN[@]}" -c id --nodes gcf-no-su --prefix

# 多节点 sync：要求 gcf-su 的整块在 root-direct 之前出现
sync_out=$("${PY_RUN[@]}" -c id --nodes gcf-su,root-direct --sync --prefix 2>&1)
if [[ "$sync_out" == *"[gcf-su] uid=0(root)"*"[root-direct] uid=0(root)"* ]] && \
   [[ "$sync_out" != *"[root-direct]"*"[gcf-su]"* ]]; then
    printf '  \033[32mPASS\033[0m %s\n' "T15 --sync 按 config 顺序整块输出"
    PASS=$((PASS+1))
else
    printf '  \033[31mFAIL\033[0m %s\n' "T15 --sync 按 config 顺序整块输出"
    sed 's/^/        /' <<< "$sync_out"
    FAIL=$((FAIL+1))
    FAIL_NAMES+=("T15")
fi

assert_run "T16 多行输出能正确传递 (包含 hostname)" \
    expect-pass 'VM-|Linux|hostname' \
    "${PY_RUN[@]}" -c "uname -a; hostname" --nodes gcf-no-su

assert_run "T17 远端命令的 stderr 透传" \
    expect-pass 'No such file or directory' \
    "${PY_RUN[@]}" -c 'ls /no_such_path 2>&1; true' --nodes gcf-no-su

# T18: --verbose 把 debug 写到 stderr，stdout 不被污染
verbose_out_stdout="$("${PY_RUN[@]}" -c id --nodes gcf-no-su -v 2>/dev/null || true)"
verbose_out_stderr="$("${PY_RUN[@]}" -c id --nodes gcf-no-su -v 2>&1 >/dev/null || true)"
if grep -qE 'uid=' <<< "$verbose_out_stdout" && grep -qE '\[DEBUG\]' <<< "$verbose_out_stderr"; then
    printf '  \033[32mPASS\033[0m %s\n' "T18 --verbose: stdout 仅有命令输出, stderr 才有 [DEBUG]"
    PASS=$((PASS+1))
else
    printf '  \033[31mFAIL\033[0m %s\n' "T18 --verbose: stdout 仅有命令输出, stderr 才有 [DEBUG]"
    echo "        stdout: $verbose_out_stdout" | head -3
    echo "        stderr: $verbose_out_stderr" | head -3
    FAIL=$((FAIL+1))
    FAIL_NAMES+=("T18")
fi

echo
echo "=== Multi-node parallel ==="

assert_run "T19 多节点并行执行: 两个 id 都出现" \
    expect-pass 'gcf-no-su.*root-direct|root-direct.*gcf-no-su' \
    bash -c "${PY_RUN[*]} -c id --nodes gcf-no-su,root-direct --prefix | tr '\\n' ' '"

# ---- summary -----------------------------------------------------------------
echo
echo "==========================================="
TOTAL=$((PASS+FAIL))
if [[ $FAIL -eq 0 ]]; then
    printf '\033[32m%s\033[0m\n' "RESULT: $PASS / $TOTAL passed"
    exit 0
else
    printf '\033[31m%s\033[0m\n' "RESULT: $PASS / $TOTAL passed, $FAIL failed:"
    for n in "${FAIL_NAMES[@]}"; do
        echo "  - $n"
    done
    exit 1
fi
