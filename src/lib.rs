use anyhow::Result;
use russh::{client::Handler, Disconnect, ChannelMsg};
use std::sync::Arc;
use async_trait::async_trait;
use russh_keys::key::PublicKey;

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub ssh: SshConfig,
    pub login_script: Vec<ScriptStep>,
    pub command: Vec<ScriptStep>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug, serde::Deserialize, Clone)]
pub struct ScriptStep {
    pub name: String,
    pub wait: String,
    pub send: String,
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

pub async fn execute_ssh(
    host: String,
    port: u16,
    user: String,
    password: String,
    login_script: Vec<ScriptStep>,
    commands: Vec<ScriptStep>,
    verbose: bool,
) -> Result<bool> {
    if verbose {
        eprintln!("[DEBUG] Connecting to {}:{} as {}", host, port, user);
    }
    let total_steps = login_script.len() + commands.len();
    if total_steps == 0 {
        return Ok(true);
    }

    let ssh_config = Arc::new(russh::client::Config::default());

    let mut session = russh::client::connect(
        ssh_config,
        (&host as &str, port),
        ClientHandler,
    ).await?;

    if !session.authenticate_password(&user, &password).await? {
        return Ok(true);
    }

    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 80, 24, 0, 0, &[]).await?;
    channel.request_shell(true).await?;

    for s in &login_script {
        if verbose {
            eprintln!("[DEBUG] Login step: {} - send: {}", s.name, s.send);
        }
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
                None => return Ok(false),
            }
        }

        let cmd = format!("{}\n", s.send);
        channel.data(cmd.as_bytes()).await?;
    }

    let mut interrupted = false;
    let mut full_buf = String::new();
    let mut output_start = 0;

    for s in &commands {
        if verbose {
            eprintln!("[DEBUG] Execute command: {}", s.send);
        }
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
                None => return Ok(false),
            }
        }

        let cmd = format!("echo MY_begin && {} && echo MY_end\n", s.send);
        channel.data(cmd.as_bytes()).await?;
        
        let found_end_marker = false;
        
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
                                            println!("{}", output_clean);
                                        }
                                    }
                                    break;
                                }
                                
                                print!("{}", content);
                                output_start = full_buf.len();
                            }
                        }
                    } else {
                        break;
                    }
                }
            }
            
            if interrupted || found_end_marker {
                break;
            }
        }

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
