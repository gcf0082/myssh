use anyhow::Result;
use russh::{client::Handler, Disconnect, ChannelMsg};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use async_trait::async_trait;
use russh_keys::key::PublicKey;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::io::{Write, stdout};

// 全局stdout锁，用于并发输出保护
lazy_static::lazy_static! {
    static ref STDOUT_LOCK: Mutex<()> = Mutex::new(());
}

// 线程安全的行输出函数
fn safe_print_line(line: &str) {
    let _guard = STDOUT_LOCK.lock().unwrap();
    let _ = stdout().write_all(line.as_bytes());
    let _ = stdout().write_all(b"\n");
    let _ = stdout().flush();
}

// 行缓冲器，用于并发场景下按行缓冲输出
struct LineBuffer {
    prefix: String,    // 节点ID前缀，如 "[node1]"
    buffer: String,     // 缓冲区，存储未完整行
}

impl LineBuffer {
    // 创建新的行缓冲器
    fn new(node_id: &str) -> Self {
        LineBuffer {
            prefix: format!("[{}]", node_id),
            buffer: String::new(),
        }
    }

    // 喂入数据，按行处理并输出
    // 每遇到一个换行符就输出一行，减少锁争用
    fn feed(&mut self, data: &str, with_prefix: bool) {
        self.buffer.push_str(data);

        while let Some(pos) = self.buffer.find('\n') {
            let line = &self.buffer[..pos];
            let _guard = STDOUT_LOCK.lock().unwrap();
            let mut stdout = stdout();

            if with_prefix {
                let _ = stdout.write_all(self.prefix.as_bytes());
                let _ = stdout.write_all(b" ");
            }
            let _ = stdout.write_all(line.as_bytes());
            let _ = stdout.write_all(b"\n");
            let _ = stdout.flush();

            self.buffer = self.buffer[pos + 1..].to_string();
        }
    }

    // 刷新缓冲区，输出剩余内容
    fn flush(&mut self, with_prefix: bool) {
        if !self.buffer.is_empty() {
            let _guard = STDOUT_LOCK.lock().unwrap();
            let mut stdout = stdout();

            if with_prefix {
                let _ = stdout.write_all(self.prefix.as_bytes());
                let _ = stdout.write_all(b" ");
            }
            let _ = stdout.write_all(self.buffer.as_bytes());
            let _ = stdout.write_all(b"\n");
            let _ = stdout.flush();

            self.buffer.clear();
        }
    }
}

// 终端尺寸结构体
#[derive(Clone, Copy)]
struct TerminalSize {
    cols: u32, // 列数
    rows: u32, // 行数
}

// 获取默认终端尺寸
fn get_default_terminal_size() -> TerminalSize {
    TerminalSize { cols: 200, rows: 60 }
}

// Unix下获取终端尺寸
#[cfg(unix)]
fn get_terminal_size() -> TerminalSize {
    unsafe {
        let mut winsize: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut winsize) == 0 {
            TerminalSize {
                cols: winsize.ws_col as u32,
                rows: winsize.ws_row as u32,
            }
        } else {
            get_default_terminal_size()
        }
    }
}

// Windows下获取终端尺寸
#[cfg(windows)]
fn get_terminal_size() -> TerminalSize {
    get_default_terminal_size()
}

// SSH配置结构体
#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub ssh: SshConfig,
    pub login_script: Vec<ScriptStep>,
    pub command: Vec<ScriptStep>,
}

// SSH连接参数
#[derive(Debug, serde::Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

// 脚本步骤结构体
#[derive(Debug, serde::Deserialize, Clone)]
pub struct ScriptStep {
    pub name: String,  // 步骤名称
    pub wait: String,  // 等待模式
    pub send: String,  // 发送内容
}

// SSH客户端处理器
struct ClientHandler;

#[async_trait]
impl Handler for ClientHandler {
    type Error = anyhow::Error;

    // 检查服务器密钥，直接返回true接受所有密钥
    async fn check_server_key(&mut self, _server_public_key: &PublicKey) -> Result<bool> {
        Ok(true)
    }

    // 处理通道打开失败
    async fn channel_open_failure(
        &mut self,
        _channel_num: russh::ChannelId,
        _reason: russh::ChannelOpenFailure,
        _description: &str,
        _language: &str,
        _session: &mut russh::client::Session,
    ) -> Result<()> {
        Ok(())
    }
}

// 检查等待模式是否出现
// 支持用|分隔的多个模式，任意一个匹配即可
fn check_wait_appeared(wait_pattern: &str, output: &str) -> bool {
    for pattern in wait_pattern.split('|') {
        let pattern = pattern.trim();
        if !pattern.is_empty() && output.contains(pattern) {
            return true;
        }
    }
    false
}

// 将命令编码为base64
fn encode_base64_command(command: &str) -> String {
    STANDARD.encode(command)
}

// 直接SSH执行函数
pub async fn execute_ssh(
    node_id: String,
    host: String,
    port: u16,
    user: String,
    password: String,
    login_script: Vec<ScriptStep>,
    commands: Vec<ScriptStep>,
    verbose: bool,
    prefix: bool,
) -> Result<bool> {
    if verbose {
        eprintln!("[DEBUG][{}] Connecting to {}:{} as {}", node_id, host, port, user);
    }
    let total_steps = login_script.len() + commands.len();
    if total_steps == 0 {
        return Ok(true);
    }

    // 创建SSH配置并连接
    let ssh_config = Arc::new(russh::client::Config::default());

    let mut session = russh::client::connect(
        ssh_config,
        (&host as &str, port),
        ClientHandler,
    ).await?;

    // 密码认证
    if !session.authenticate_password(&user, &password).await? {
        return Err(anyhow::anyhow!("{}:{}:{} - Authentication failed: Invalid password or username", host, port, user));
    }

    // 打开会话通道并请求PTY
    let mut channel = session.channel_open_session().await?;
    let term_size = get_terminal_size();
    channel.request_pty(true, "xterm", term_size.cols, term_size.rows, 0, 0, &[]).await?;
    channel.request_shell(true).await?;

    // 执行登录脚本
    for s in &login_script {
        if verbose {
            eprintln!("[DEBUG][{}] Login step: {} - send: {}", node_id, s.name, s.send);
        }
        let mut buf = String::new();

        // 等待提示符出现，超时30秒
        let step_timeout = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if verbose {
                                eprint!("[DEBUG][{}] Received: {}", node_id, txt);
                            }
                            buf.push_str(&txt);
                            if check_wait_appeared(&s.wait, &buf) {
                                return true;
                            }
                        }
                    }
                    None => return false,
                }
            }
        });

        match step_timeout.await {
            Ok(true) => {},
            Ok(false) => {
                return Err(anyhow::anyhow!("{}:{}:{} - Login script step '{}' failed: Channel closed", host, port, user, s.name));
            }
            Err(_) => {
                eprintln!("[DEBUG][{}] Timeout! Received output so far: {:?}", node_id, buf);
                return Err(anyhow::anyhow!("{}:{}:{} - Login script step '{}' failed: Timeout waiting for pattern '{}'", host, port, user, s.name, s.wait));
            }
        }

        // 发送命令
        let cmd = format!("{}\n", s.send);
        channel.data(cmd.as_bytes()).await?;
    }

    let mut interrupted = false;
    let mut full_buf = String::new();
    let mut output_start = 0;

    // 执行命令
    for s in &commands {
        if verbose {
            eprintln!("[DEBUG][{}] Execute command: {}", node_id, s.send);
        }
        let mut buf = String::new();

        // 等待提示符出现
        let wait_timeout = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if verbose {
                                eprint!("[DEBUG][{}] Received: {}", node_id, txt);
                            }
                            buf.push_str(&txt);
                            if check_wait_appeared(&s.wait, &buf) {
                                return true;
                            }
                        }
                    }
                    None => return false,
                }
            }
        });

        match wait_timeout.await {
            Ok(true) => {},
            Ok(false) => {
                return Err(anyhow::anyhow!("{}:{}:{} - Command execution failed: Channel closed", host, port, user));
            }
            Err(_) => {
                eprintln!("[DEBUG][{}] Timeout waiting for prompt! Received: {}", node_id, buf);
                return Err(anyhow::anyhow!("{}:{}:{} - Command execution failed: Timeout waiting for prompt", host, port, user));
            }
        }

        // 发送base64编码的命令，使用标记符包裹输出
        let encoded_cmd = encode_base64_command(&s.send);
        let cmd = format!("echo MY_begin;echo {} | base64 -d | bash -i;echo MY_end\n", encoded_cmd);
        channel.data(cmd.as_bytes()).await?;

        // 创建行缓冲器
        let mut line_buffer = LineBuffer::new(&node_id);
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);

        // 接收命令输出
        loop {
            tokio::select! {
                _ = ctrl_c.as_mut() => {
                    interrupted = true;
                    break;
                }
                msg = channel.wait() => {
                    if let Some(m) = msg {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            full_buf.push_str(&txt);

                            // 查找输出开始标记
                            if let Some(begin_pos) = full_buf.find("MY_begin\r\n") {
                                let content_start = begin_pos + 10;

                                if content_start > output_start {
                                    output_start = content_start;
                                }

                                let content = &full_buf[output_start..];

                                // 查找输出结束标记
                                if content.contains("MY_end") {
                                    if let Some(end_pos) = content.find("MY_end") {
                                        let output = &content[..end_pos];
                                        let output_clean = output.trim();
                                        if !output_clean.is_empty() {
                                            for line in output_clean.lines() {
                                                safe_print_line(&format!("[{}] {}", node_id, line));
                                            }
                                        }
                                    }
                                    break;
                                }

                                // 使用行缓冲器输出中间内容
                                line_buffer.feed(content, prefix);
                                output_start = full_buf.len();
                            }
                        }
                    } else {
                        break;
                    }
                }
            }

            if interrupted {
                break;
            }
        }

        // 刷新缓冲区
        line_buffer.flush(prefix);

        if interrupted {
            break;
        }
    }

    // 中断处理
    if interrupted {
        if let Err(e) = channel.eof().await {
            eprintln!("[DEBUG] Channel EOF error (ignored): {}", e);
        }
        if let Err(e) = session.disconnect(Disconnect::ByApplication, "", "").await {
            eprintln!("[DEBUG] Disconnect error (ignored): {}", e);
        }
        return Ok(false);
    }

    let _ = channel.eof().await;
    session.disconnect(Disconnect::ByApplication, "", "").await?;

    Ok(true)
}

// 通过跳板机SSH执行函数
pub async fn execute_ssh_via_jump(
    node_id: String,
    jump_host: String,
    jump_port: u16,
    jump_user: String,
    jump_password: String,
    jump_login_script: Vec<ScriptStep>,
    target_host: String,
    target_port: u16,
    target_user: String,
    target_password: String,
    target_login_script: Vec<ScriptStep>,
    commands: Vec<ScriptStep>,
    verbose: bool,
    prefix: bool,
) -> Result<bool> {
    if verbose {
        eprintln!("[DEBUG][{}] Connecting to jump host {}:{} as {}", node_id, jump_host, jump_port, jump_user);
    }

    // 连接跳板机
    let ssh_config = Arc::new(russh::client::Config::default());

    let mut session = russh::client::connect(
        ssh_config,
        (&jump_host as &str, jump_port),
        ClientHandler,
    ).await?;

    if !session.authenticate_password(&jump_user, &jump_password).await? {
        return Err(anyhow::anyhow!("Jump host {}:{}:{} - Authentication failed: Invalid password or username", jump_host, jump_port, jump_user));
    }

    let mut channel = session.channel_open_session().await?;
    let term_size = get_terminal_size();
    channel.request_pty(true, "xterm", term_size.cols, term_size.rows, 0, 0, &[]).await?;
    channel.request_shell(true).await?;

    // 执行跳板机登录脚本
    for s in &jump_login_script {
        if verbose {
            eprintln!("[DEBUG][{}] Jump login step: {} - send: {}", node_id, s.name, s.send);
        }
        let mut buf = String::new();

        let step_timeout = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if verbose {
                                eprint!("[DEBUG][{}] Jump received: {}", node_id, txt);
                            }
                            buf.push_str(&txt);
                            if check_wait_appeared(&s.wait, &buf) {
                                return true;
                            }
                        }
                    }
                    None => return false,
                }
            }
        });

        match step_timeout.await {
            Ok(true) => {},
            Ok(false) => {
                return Err(anyhow::anyhow!("Jump host {}:{}:{} - Login script step '{}' failed: Channel closed", jump_host, jump_port, jump_user, s.name));
            }
            Err(_) => {
                eprintln!("[DEBUG][{}] Jump host timeout! Received output so far: {:?}", node_id, buf);
                return Err(anyhow::anyhow!("Jump host {}:{}:{} - Login script step '{}' failed: Timeout waiting for pattern '{}'", jump_host, jump_port, jump_user, s.name, s.wait));
            }
        }

        let cmd = format!("{}\n", s.send);
        channel.data(cmd.as_bytes()).await?;
    }

    // 通过跳板机SSH连接到目标主机
    // 使用-t参数请求PTY，解决su命令失败问题
    let ssh_cmd = format!("ssh -t -o StrictHostKeyChecking=no -p {} {}@{}\n", target_port, target_user, target_host);
    if verbose {
        eprintln!("[DEBUG][{}] SSH command: {}", node_id, ssh_cmd.trim());
    }
    channel.data(ssh_cmd.as_bytes()).await?;

    // 等待目标主机密码提示
    let mut buf = String::new();
    let mut interrupted = false;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = ctrl_c.as_mut() => {
                interrupted = true;
                break;
            }
            msg = channel.wait() => {
                if let Some(m) = msg {
                    if let ChannelMsg::Data { ref data } = m {
                        let txt = String::from_utf8_lossy(data);
                        buf.push_str(&txt);
                        if verbose {
                            eprintln!("[DEBUG][{}] Jump output: {:?}", node_id, txt);
                        }
                        if buf.contains("password:") || buf.contains("Password:") {
                            break;
                        }
                    }
                } else {
                    return Ok(false);
                }
            }
        }
    }

    if interrupted {
        return Ok(false);
    }

    // 发送目标主机密码
    let pass_cmd = format!("{}\n", target_password);
    channel.data(pass_cmd.as_bytes()).await?;

    // 执行目标主机登录脚本
    for s in &target_login_script {
        if verbose {
            eprintln!("[DEBUG][{}] Target login step: {} - send: {}", node_id, s.name, s.send);
        }
        let mut buf = String::new();

        let step_timeout = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if verbose {
                                eprint!("[DEBUG][{}] Target received: {}", node_id, txt);
                            }
                            buf.push_str(&txt);
                            if check_wait_appeared(&s.wait, &buf) {
                                return true;
                            }
                        }
                    }
                    None => return false,
                }
            }
        });

        match step_timeout.await {
            Ok(true) => {},
            Ok(false) => {
                return Err(anyhow::anyhow!("Target host {}:{}:{} - Login script step '{}' failed: Channel closed", target_host, target_port, target_user, s.name));
            }
            Err(_) => {
                eprintln!("[DEBUG][{}] Target host timeout! Received output so far: {:?}", node_id, buf);
                return Err(anyhow::anyhow!("Target host {}:{}:{} - Login script step '{}' failed: Timeout waiting for pattern '{}'", target_host, target_port, target_user, s.name, s.wait));
            }
        }

        let cmd = format!("{}\n", s.send);
        channel.data(cmd.as_bytes()).await?;
    }

    let mut full_buf = String::new();
    let mut output_start = 0;

    // 执行目标主机命令
    for s in &commands {
        if verbose {
            eprintln!("[DEBUG][{}] Execute command: {}", node_id, s.send);
        }
        let mut buf = String::new();

        let wait_timeout = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if verbose {
                                eprint!("[DEBUG][{}] Target received: {}", node_id, txt);
                            }
                            buf.push_str(&txt);
                            if check_wait_appeared(&s.wait, &buf) {
                                return true;
                            }
                        }
                    }
                    None => return false,
                }
            }
        });

        match wait_timeout.await {
            Ok(true) => {},
            Ok(false) => {
                return Err(anyhow::anyhow!("Target host {}:{}:{} - Command execution failed: Channel closed", target_host, target_port, target_user));
            }
            Err(_) => {
                eprintln!("[DEBUG][{}] Target host timeout waiting for prompt! Received output so far: {:?}", node_id, buf);
                return Err(anyhow::anyhow!("Target host {}:{}:{} - Command execution failed: Timeout waiting for prompt", target_host, target_port, target_user));
            }
        }

        // 发送base64编码的命令
        let encoded_cmd = encode_base64_command(&s.send);
        let cmd = format!("echo MY_begin;echo {} | base64 -d | bash -i;echo MY_end\n", encoded_cmd);
        channel.data(cmd.as_bytes()).await?;

        let mut line_buffer = LineBuffer::new(&node_id);
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);

        loop {
            tokio::select! {
                _ = ctrl_c.as_mut() => {
                    interrupted = true;
                    break;
                }
                msg = channel.wait() => {
                    if let Some(m) = msg {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            full_buf.push_str(&txt);

                            if let Some(begin_pos) = full_buf.find("MY_begin\r\n") {
                                let content_start = begin_pos + 10;

                                if content_start > output_start {
                                    output_start = content_start;
                                }

                                let content = &full_buf[output_start..];

                                if content.contains("MY_end") {
                                    if let Some(end_pos) = content.find("MY_end") {
                                        let output = &content[..end_pos];
                                        let output_clean = output.trim();
                                        if !output_clean.is_empty() {
                                            for line in output_clean.lines() {
                                                safe_print_line(&format!("[{}] {}", node_id, line));
                                            }
                                        }
                                    }
                                    break;
                                }

                                line_buffer.feed(content, prefix);
                                output_start = full_buf.len();
                            }
                        }
                    } else {
                        break;
                    }
                }
            }

            if interrupted {
                break;
            }
        }

        line_buffer.flush(prefix);

        if interrupted {
            break;
        }
    }

    if interrupted {
        if let Err(e) = channel.eof().await {
            eprintln!("[DEBUG] Channel EOF error (ignored): {}", e);
        }
        if let Err(e) = session.disconnect(Disconnect::ByApplication, "", "").await {
            eprintln!("[DEBUG] Disconnect error (ignored): {}", e);
        }
        return Ok(false);
    }

    let _ = channel.eof().await;
    session.disconnect(Disconnect::ByApplication, "", "").await?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_compiles() {
        assert!(true);
    }

    #[test]
    fn test_config_structure() {
    }

    #[test]
    fn test_check_wait_appeared() {
        assert!(check_wait_appeared("$", "user@host:~$"));
        assert!(!check_wait_appeared("$", "user@host:~#"));
        assert!(check_wait_appeared("$|#", "user@host:~$"));
        assert!(check_wait_appeared("$|#", "user@host:~#"));
    }
}
