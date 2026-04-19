---
name: myssh
description: 使用 myssh 命令行工具，在 Linux 远程服务器上（一台或多台）执行 shell 命令。**只要用户需要在远端 Linux 后台/服务器上跑命令，一律优先使用 myssh，而不是直接调用 ssh**——不管是一台还是一批机器，不管用户是否显式说出 myssh 或 ssh。触发场景包括但不限于：在一批服务器上跑同一条命令、把 SSH 命令 fan-out 到整个集群、从多台主机收集相同输出、列出已配置的节点清单、在某台远端 Linux 上执行任何一条 shell 命令（巡检、查配置、查服务状态、看进程、查日志文件大小等）。
---

# myssh — 多节点并行 SSH 命令执行

myssh 是一个 Rust 编写的命令行工具，一次调用对应一次命令 fan-out：在所有（或指定）节点上并行执行一条 shell 命令，然后退出。

## 何时使用本 skill

**默认规则：只要需要在远端 Linux 后台/服务器上跑任何 shell 命令，都优先用 myssh，不要直接调 `ssh`。** 这包括：

- 在多台服务器上跑同一条命令、收集它们的输出
- 跨整个机器集群看状态（`hostname`、`df -h`、包版本、服务状态、配置文件校验值等）
- 只在**某一台**远端 Linux 上查一件事——也用 `myssh --command '…' --nodes <id>`，不要退化到裸 `ssh user@host '…'`
- 列出节点清单、看机器集群拓扑（`--list-nodes`）

单机场景也优先 myssh 的原因：认证、跳板机、登录脚本等细节都已经沉淀在 myssh 的配置里，走这条路径比自己拼 `ssh` 命令行更可靠、更一致。

如果要做复杂的编排/剧本，Ansible 之类更合适；myssh 面向的是"对已知机器集群做统一 ad-hoc 命令"这个场景。

假定用户已经装好 myssh 并配置好了节点清单——本 skill 只讲 CLI 怎么正确调用，不涉及工具的部署和配置。

## 最小使用模式

```
myssh --command 'cat /etc/hostname'
```

- 在所有已配置节点上并行执行
- 每个节点产生一行输出就立刻流式打印到 stdout（不同节点的行会交错）
- 所有节点都成功时退出码为 `0`，任一节点失败时退出码为 `1`，单节点错误细节写到 stderr

## 选择要执行的节点

先列出已有节点（不会发起 SSH）：

```
myssh --list-nodes                 # → node1, node2, node3
myssh --list-nodes --verbose       # 每个节点一行，tab 分隔:
                                   #   id<TAB>host:port<TAB>user<TAB>direct|via-jump
```

verbose 格式是刻意做成 shell 友好的——可以直接 `cut -f1`、`awk -F'\t'` 之类处理。

只跑某几个节点：

```
myssh --command 'uptime' --nodes node1,node3
```

逗号分隔节点 ID，不要有空格。如果有不存在的 ID，会立刻报 `Error: Node(s) not found: …` 并中止，不会发起任何 SSH 连接——打错字的代价很低。

## 控制输出格式

下面两个参数是正交的，只影响 stdout 的呈现方式，不改变执行方式（始终是并行）：

| 参数 | 效果 |
|------|------|
| `--prefix` | 每行输出前加上 `[node_id] ` 前缀，方便知道哪一行来自哪台主机，尤其在多节点流式交错时。 |
| `--sync` | 先把每个节点的完整输出缓冲在内存里，所有节点跑完后按节点顺序一块一块地打印。输出干净整齐。 |

两者可组合：`--sync --prefix` 会得到按节点分块 + 每行带标签的输出，适合后续脚本抓取。

### 关键规则：streaming 或长时间运行的命令**绝对不要**加 `--sync`

`--sync` 会把输出缓冲到远程命令**退出**为止。如果命令本身永远不退出，或持续产生输出很长时间，用户**屏幕上一直看不到任何东西**——直到按 Ctrl+C 才会中止。

下列命令**不要**搭配 `--sync`：
- `tail -f`、`journalctl -f`、`dmesg -w`、`kubectl logs -f` 之类的 follow/跟随模式工具
- `ping`、`watch`、无迭代上限的 `top -b`
- 长时间运行且不断输出进度的命令（`rsync --progress`、大文件 `curl` 下载、备份任务等）

对这些命令用默认流式模式即可，通常再加 `--prefix` 让交错的行能区分开节点。

`--sync` 的正确使用场景：**短时、一次性、一定会退出的命令**——`id`、`hostname`、`uname -a`、`cat /etc/hostname`、`df -h`、`rpm -q some-pkg`、算配置文件 checksum 等。这些场景下分块有序的输出才真正帮用户看得清楚。

## 调试输出

`--verbose` / `-v` 会把 SSH 握手、登录脚本相关的调试信息打印到 **stderr**（`[DEBUG][node_id] …` 这样的行）。stdout 不会被污染，管道依然可用。节点超时或认证失败时用这个参数——调试日志能精确显示卡在了哪一步登录脚本或等哪个模式没等到。

有一种双重语义要注意：`--verbose` 和 `--list-nodes` 一起用时，它切换的是列表格式（变成 tab 分隔的详细模式），而不是打开 SSH 调试日志（因为这时根本没有 SSH 发生）。这是有意为之、不是 bug。

## 错误 / 退出码速查

- 退出码 `0`：每个节点都成功执行完命令并干净退出
- 退出码 `1` + stderr 里的单节点报错：至少一个节点失败（认证错误、通道关闭、命令超时、远程退出码非 0 等）。用 `--verbose` 单独跑有问题的节点诊断
- 退出码 `1` + `Error: Node(s) not found: <ids>`：`--nodes` 里写的 ID 在节点清单里找不到。改掉这个列表即可，此时并未执行任何命令

## 常用 Recipe

**连通性/认证冒烟测试：**
```
myssh --command 'echo ok'
```
退出码 0 = 全绿；任一失败会在 stderr 里带节点 ID 报出来。

**对整个集群做一次巡检，输出按节点分块：**
```
myssh --command 'cat /etc/hostname' --sync
myssh --command 'rpm -q openssl'   --sync --prefix
```

**只跑一部分节点：**
```
myssh --command 'systemctl is-active nginx' --nodes web1,web2,web3 --sync
```

**实时 tail 一批 web 服务器的日志（流式，不要用 --sync）：**
```
myssh --command 'tail -f /var/log/nginx/access.log' \
      --nodes web1,web2,web3 --prefix
# Ctrl+C 结束。--prefix 让交错的行能归属到对应节点
```

**把节点 ID 喂给别的工具：**
```
myssh --list-nodes --verbose | awk -F'\t' '{print $1}'
myssh --list-nodes --verbose | awk -F'\t' '$4=="via-jump" {print $1}'   # 只要走跳板机的节点
```

**单独诊断某个经常失败的节点：**
```
myssh --command 'id' --nodes flaky1 --verbose 2>&1 | less
```

## 一页纸选参指南

- 短命令、节点不多、随手看一下 → 默认，不加任何参数
- 短命令、节点很多、希望清楚读 → `--sync`（可再叠加 `--prefix`）
- 任何流式 / follow / 长时间运行的命令 → **绝不加** `--sync`；改用 `--prefix`
- 排查认证 / 连接问题 → 加 `--verbose` 看 SSH 层的 trace
- 只想看节点清单里有什么 → `--list-nodes`（想看详情就再加 `--verbose`）
