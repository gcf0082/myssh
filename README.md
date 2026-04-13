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
```

## 配置文件

配置文件位于项目根目录的 `config.yaml`。

### 完整配置示例

```yaml
# ==================== 全局默认配置 ====================
defaults:
  # 默认 SSH 端口、用户名、密码（节点可覆盖）
  port: 22
  user: "user"
  password: "default_password"
  # 默认登录脚本（所有节点共用，节点可通过 login_script 字段追加或覆盖）
  login_script:
    - name: "SSH登录"
      wait: "$"
      send: "su - root"
    - name: "输入密码"
      wait: "Password:"
      send: "{{password}}"   # 占位符，实际执行时用节点最终密码替换

# ==================== 节点列表 ====================
nodes:
  # 节点1：完全使用默认配置（复用脚本、密码）
  - id: "node1"
    host: "1.1.1.1"

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
```

### 配置说明

#### 全局默认配置 (`defaults`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `port` | u16 | 默认 SSH 端口（默认 22） |
| `user` | String | 默认用户名 |
| `password` | String | 默认密码 |
| `login_script` | Vec | 默认登录脚本步骤 |

#### 节点配置 (`nodes`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | String | 节点唯一标识 |
| `host` | String | 主机地址 |
| `port` | u16 | SSH 端口（可覆盖默认值） |
| `user` | String | 用户名（可覆盖默认值） |
| `password` | String | 密码（可覆盖默认值） |
| `login_script` | Vec | 完全覆盖默认登录脚本 |
| `login_script_append` | Vec | 在默认脚本后追加步骤 |

### 占位符

在 `login_script` 中使用 `{{password}}`，执行时会自动替换为节点的最终密码（节点自己的 password 或 defaults 中的 password）。

## 运行测试

```bash
cargo test
```