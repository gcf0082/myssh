use anyhow::Result;
use clap::Parser;
use myssh::{execute_ssh, ScriptStep};

#[derive(Parser, Debug)]
#[command(name = "myssh")]
struct Cli {
    #[arg(short, long, help = "Command to execute (overrides config.command)")]
    command: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_str = std::fs::read_to_string("config.toml")?;
    let config: myssh::Config = toml::from_str(&config_str)?;

    let commands_to_run: Vec<ScriptStep> = if let Some(cmd) = cli.command {
        let wait = config
            .command
            .first()
            .map(|s| s.wait.clone())
            .unwrap_or_else(|| "$|#".to_string());
        vec![ScriptStep {
            name: "cli".to_string(),
            wait,
            send: cmd,
        }]
    } else {
        config.command.clone()
    };

    execute_ssh(
        config.ssh.host,
        config.ssh.port,
        config.ssh.user,
        config.ssh.password,
        config.login_script,
        commands_to_run,
    ).await?;

    Ok(())
}
