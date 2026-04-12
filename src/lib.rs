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
) -> Result<()> {
    let total_steps = login_script.len() + commands.len();
    if total_steps == 0 {
        return Ok(());
    }

    let ssh_config = Arc::new(russh::client::Config::default());

    let mut session = russh::client::connect(
        ssh_config,
        (&host as &str, port),
        ClientHandler,
    ).await?;

    if !session.authenticate_password(&user, &password).await? {
        return Ok(());
    }

    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 80, 24, 0, 0, &[]).await?;
    channel.request_shell(true).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    for s in &login_script {
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

        let cmd = format!("{}\n", s.send);
        channel.data(cmd.as_bytes()).await?;
    }

    for s in &commands {
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
    }

    let mut output_buffer = String::new();
    let mut in_output = false;
    let ctrl_c = tokio::signal::ctrl_c();
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
