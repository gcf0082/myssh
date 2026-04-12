# MySSH - 远程 SSH 日志监控工具

这是一个用 Rust 编写的 SSH 连接工具，支持类似 xshell 的登录脚本配置，可以实时监控远程服务器的日志文件。

## 功能特性

- 通过 SSH 连接到远程服务器
- 支持配置文件设置登录脚本
- 智能提示符检测判断命令执行状态
- 自动切换到 root 用户
- 实时监控日志文件
- 支持 Ctrl+C 优雅退出
- 跨平台支持（Linux 和 Windows）

## 配置文件

配置文件 `config.toml` 格式：

```toml
# 服务器连接配置
[ssh]
host = "1.1.1.1"
port = 22
user = "user1"
password = "password"

# 登录脚本配置
# wait: 等待出现的字符串（多个用 | 分隔）
# send: 等待成功后发送的命令

[[script]]
name = "等待 SSH 登录成功"
wait = "$|#"
send = "su -"

[[script]]
name = "输入 root 密码"
wait = "Password:"
send = "password"

[[script]]
name = "等待 root 提示符并启动日志监控"
wait = "#"
send = "tail -F /var/log/messages"

# 可以添加更多步骤
# [[script]]
# name = "显示系统信息"
# wait = "#"
# send = "uname -a"
```

## 编译和运行

### 编译

```bash
cargo build --release
```

编译后的可执行文件位于 `target/release/myssh`

### 运行

```bash
./target/release/myssh
```

### 在 Windows 上编译和运行

```bash
cargo build --release
.\target\release\myssh.exe
```

## 工作原理

1. 程序读取 `config.toml` 配置文件
2. 连接到 SSH 服务器
3. 执行脚本步骤：
   - 等待指定的提示符出现
   - 检测到后发送对应的命令
4. 循环监控日志输出
5. 按 Ctrl+C 退出

## 提示符检测规则

- 使用 `|` 分隔多个等待字符串，如 `"$|#"` 表示等待 `$` 或 `#` 出现
- 检测到任何匹配的字符串后自动发送命令
- 每个步骤执行完成后自动执行下一步

## 技术栈

- **Rust**: 2021 edition
- **异步运行时**: tokio
- **SSH 库**: russh
- **配置解析**: toml + serde
- **异步特质**: async-trait

## 许可证

MIT License
