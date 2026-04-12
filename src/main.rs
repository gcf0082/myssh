use anyhow::Result;
use russh::{client::Handler, Disconnect, ChannelMsg};
use serde::Deserialize;
use std::sync::Arc;
use tokio::signal;
use async_trait::async_trait;
use russh_keys::key::PublicKey;

#[derive(Debug, Deserialize)]
struct Config {
    ssh: SshConfig,
    script: Vec<ScriptStep>,
}

#[derive(Debug, Deserialize)]
struct SshConfig {
    host: String,
    port: u16,
    user: String,
    password: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ScriptStep {
    name: String,
    wait: String,
    send: String,
}

struct ClientHandler;

#[async_trait]
impl Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, _server_public_key: &PublicKey) -> Result<bool> {
        Ok(true)
    }

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

fn check_wait_appeared(wait_pattern: &str, output: &str) -> bool {
    for pattern in wait_pattern.split('|') {
        let pattern = pattern.trim();
        if !pattern.is_empty() && output.contains(pattern) {
            return true;
        }
    }
    false
}


#[tokio::main]
async fn main() -> Result<()> {
    let config_str = std::fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;

    let total_steps = config.script.len();
    if total_steps == 0 {
        return Ok(());
    }

    let ssh_config = Arc::new(russh::client::Config::default());

    let mut session = russh::client::connect(
        ssh_config,
        (&config.ssh.host as &str, config.ssh.port),
        ClientHandler,
    ).await?;

    if !session.authenticate_password(&config.ssh.user, &config.ssh.password).await? {
        return Ok(());
    }

    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 80, 24, 0, 0, &[]).await?;
    channel.request_shell(true).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 发送所有脚本步骤的命令
    for step_idx in 0..total_steps {
        let s = &config.script[step_idx];
        let mut buf = String::new();
        
        loop {
            match channel.wait().await {
                Some(m) => {
                    if let ChannelMsg::Data { ref data } = m {
                        let txt = String::from_utf8_lossy(data);
                        buf.push_str(&txt);
                        if check_wait_appeared(&s.wait, &buf) {
                            break;
                        }
                    }
                }
                None => return Ok(()),
            }
        }

        if step_idx == total_steps - 1 {
            let cmd = format!("echo MY_begin && {} && echo MY_end\n", s.send);
            channel.data(cmd.as_bytes()).await?;
            loop {
                match channel.wait().await {
                    Some(m) => {
                        if let ChannelMsg::Data { ref data } = m {
                            let txt = String::from_utf8_lossy(data);
                            if txt.contains("MY_end") {
                                break;
                            }
                        }
                    }
                    None => return Ok(()),
                }
            }
        } else {
            let cmd = format!("{}\n", s.send);
            channel.data(cmd.as_bytes()).await?;
        }
    }

    let mut output_buffer = String::new();
    let mut in_output = false;
    let ctrl_c = signal::ctrl_c();
    tokio::pin!(ctrl_c);
    
    loop {
        tokio::select! {
            _ = ctrl_c.as_mut() => {
                break;
            }
            msg = channel.wait() => {
                if let Some(m) = msg {
                    if let ChannelMsg::Data { ref data } = m {
                        let txt = String::from_utf8_lossy(data);
                        output_buffer.push_str(&txt);
                        
                        if !in_output {
                            if let Some(pos) = txt.find("MY_begin") {
                                in_output = true;
                                let rest = &txt[pos + 8..];
                                if !rest.is_empty() {
                                    print!("{}", rest);
                                }
                            }
                        } else {
                            if txt.contains("MY_end") {
                                if let Some(pos) = txt.find("MY_end") {
                                    let before_end = &txt[..pos];
                                    if !before_end.is_empty() {
                                        print!("{}", before_end);
                                    }
                                }
                                break;
                            } else {
                                print!("{}", txt);
                            }
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }

    let _ = channel.eof().await;
    session.disconnect(Disconnect::ByApplication, "", "").await?;

    Ok(())
}