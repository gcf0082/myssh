# myssh

Rust SSH 自动化工具，通过 `russh` 并行连接多台远程服务器并执行脚本化命令。

## 安装

```bash
cargo build
```

## 使用方法

```bash
# 在所有节点执行命令
cargo run -- --command "ls -la"

# 在指定节点执行命令
cargo run -- --command "hostname" --nodes node1,node3

# 简短参数
cargo run -- -c "pwd" -n node2

# 使用 --prefix 参数添加节点前缀
cargo run -- -c "hostname" --prefix

# 输出示例（使用 --prefix）：
# [node1] server1.example.com
# [node2] server2.example.com
# [node3] server3.example.com

# 显示调试信息
cargo run -- -c "uptime" --verbose

# 交互式模式（类 bash 循环输入）
cargo run -- --interactive
# 在交互式模式下，可以连续输入命令：
# myssh> ls -la
# myssh> pwd
# myssh> hostname
# myssh> exit  # 或 quit，或按 Ctrl+D 退出
```

## 命令行参数

| 参数 | 简写 | 说明 | 必填 |
|------|------|------|------|
| `--command` | `-c` | 要执行的命令（非交互式模式必填） | 否* |
| `--nodes` | `-n` | 逗号分隔的节点 ID 列表（不指定则所有节点） | 否 |
| `--prefix` | | 在每行输出前添加节点前缀 `[node_id]` | 否 |
| `--verbose` | `-v` | 显示调试信息 | 否 |
| `--interactive` | | 交互式模式（循环接收命令输入，类似 bash） | 否 |

*注：在非交互式模式下，`--command` 为必填参数。在交互式模式下，不需要指定 `--command`。

## 配置文件

配置文件位于项目根目录的 `config.yaml`。

### 完整配置示例

```yaml
# ==================== 全局默认配置 ====================
defaults:
  # 默认 SSH 端口（节点可覆盖）
  port: 22
  # 默认 SSH 用户名（节点可覆盖）
  user: "user"
  # 默认 SSH 密码（节点可覆盖）
  password: "default_password"
  # 默认登录脚本（所有节点共用，节点可通过 login_script 字段追加或覆盖）
  login_script:
    - name: "SSH登录"
      wait: "$"
      send: "su - root"
    - name: "输入密码"
      wait: "Password:"
      send: "{{password}}"   # 占位符，实际执行时用节点最终密码替换
  # 全局跳板机开关，默认为 false
  use_jump: false
  # 执行命令后等待的提示符，用于判断命令执行完成（默认为 "$|#"）
  # 支持使用 | 分隔多个可能的提示符
  # command_wait: "$|#"

# ==================== 跳板机配置 ====================
jump:
  host: "10.0.0.1"           # 跳板机 IP
  port: 22
  user: "jumphost_user"
  password: "jump_password"
  # 跳板机自身的登录后操作（可选）
  login_script:
    - name: "切换到跳板机root"
      wait: "$"
      send: "sudo -i"

# ==================== 节点列表 ====================
nodes:
  # 节点1：完全使用默认配置（复用脚本、密码）
  - id: "node1"                # 节点唯一标识符，用于 --nodes 参数指定节点
    host: "1.1.1.1"            # 节点 IP 地址或主机名
    # port: 22                  # 可选：SSH 端口，不指定则使用 defaults.port
    # user: "user"              # 可选：SSH 用户名，不指定则使用 defaults.user
    # password: "..."          # 可选：SSH 密码，不指定则使用 defaults.password
    # login_script: [...]      # 可选：完全覆盖 defaults.login_script
    # login_script_append: [...] # 可选：在默认登录脚本后追加步骤
    # use_jump: false           # 可选：是否通过跳板机连接，不指定则使用 defaults.use_jump

  # 节点2：复用默认脚本，但使用独立密码
  - id: "node2"
    host: "1.1.1.2"
    password: "node2_specific_pass"

  # 节点3：复用默认脚本，但追加额外的脚本步骤
  - id: "node3"
    host: "1.1.1.3"
    login_script_append:       # 在默认脚本后追加步骤
      - name: "执行特定命令"
        wait: "#"
        send: "ifconfig"

  # 节点4：完全覆盖默认脚本，使用自己的登录流程
  - id: "node4"
    host: "1.1.1.4"
    login_script:              # 完全覆盖 defaults.login_script
      - name: "直接 SSH"
        wait: "$"
        send: "sudo -i"
      - name: "sudo 密码"
        wait: "password for"
        send: "sudo_pass"

  # 节点5：通过跳板机连接（内网节点）
  - id: "node5"
    host: "192.168.1.10"      # 内网 IP
    use_jump: true             # 该节点走跳板机

  # 节点6：不走跳板机（管理网可直连）
  - id: "node6"
    host: "2.2.2.2"
    use_jump: false            # 明确指定不走跳板机
```

### 配置说明

#### 全局默认配置 (`defaults`)

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `port` | u16 | 默认 SSH 端口 | 22 |
| `user` | String | 默认用户名 | "" |
| `password` | String | 默认密码 | "" |
| `login_script` | Vec | 默认登录脚本步骤 | [] |
| `use_jump` | bool | 全局跳板机开关 | false |
| `command_wait` | String | 执行命令后等待的提示符 | "$|#" |

#### 跳板机配置 (`jump`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `host` | String | 跳板机主机地址 |
| `port` | u16 | SSH 端口 |
| `user` | String | 用户名 |
| `password` | String | 密码 |
| `login_script` | Vec | 跳板机登录脚本（可选） |

#### 节点配置 (`nodes`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | String | 节点唯一标识（必填） |
| `host` | String | 主机地址（必填） |
| `port` | u16 | SSH 端口（可覆盖默认值） |
| `user` | String | 用户名（可覆盖默认值） |
| `password` | String | 密码（可覆盖默认值） |
| `login_script` | Vec | 完全覆盖默认登录脚本 |
| `login_script_append` | Vec | 在默认脚本后追加步骤 |
| `use_jump` | bool | 是否使用跳板机（可覆盖默认值） |

### 登录脚本步骤说明

每个登录脚本步骤包含以下字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | String | 步骤名称（用于调试和错误提示） |
| `wait` | String | 等待出现的字符串（支持使用 `|` 分隔多个可能的匹配） |
| `send` | String | 要发送的命令 |

### 占位符

在 `login_script` 中使用 `{{password}}`，执行时会自动替换为节点的最终密码（节点自己的 password 或 defaults 中的 password）。

### command_wait 说明

`command_wait` 参数用于判断命令执行完成，默认值为 `"$|#"`，表示等待普通用户提示符 `$` 或 root 用户提示符 `#`。

特殊情况下可以自定义，例如：
- 对于 Windows 系统：`">"`
- 对于自定义提示符：`"custom_prompt|$|#"`

支持使用 `|` 分隔多个可能的提示符，只要匹配其中一个即认为命令执行完成。

## 错误处理

工具会检测并报告以下错误：

1. **节点不存在**：使用 `--nodes` 指定了不存在的节点 ID
   ```
   Error: Node(s) not found: node99, node100
   ```

2. **认证失败**：密码或用户名错误
   ```
   Error: 192.168.1.1:22:root - Authentication failed: Invalid password or username
   ```

3. **登录脚本步骤失败**：
   - 超时（30秒内未匹配到等待字符）
     ```
     Error: 192.168.1.1:22:root - Login script step '输入密码' failed: Timeout waiting for pattern 'Password:'
     ```
   - 通道关闭
     ```
     Error: 192.168.1.1:22:root - Login script step 'SSH登录' failed: Channel closed
     ```

4. **命令执行失败**：
   ```
   Error: 192.168.1.1:22:root - Command execution failed: Timeout waiting for prompt
   ```

5. **跳板机错误**：
   ```
   Error: Jump host 10.0.0.1:22:jumper - Authentication failed: Invalid password or username
   ```

## 运行测试

```bash
cargo test
```

## 架构说明

- **多节点并行执行**：所有节点连接和命令执行都是并行进行的
- **超时控制**：登录脚本步骤和命令执行都有 30 秒超时限制
- **Ctrl+C 支持**：可以随时按 Ctrl+C 中断所有任务
- **跳板机支持**：通过 jump 配置支持先连接跳板机，再连接目标节点
