use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;

#[derive(Debug, serde::Deserialize)]
struct Config {
    pub nodes: Vec<NodeConfig>,
    pub login_script: Vec<myssh::ScriptStep>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct NodeConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Parser, Debug)]
#[command(name = "myssh")]
struct Cli {
    #[arg(short, long, help = "Command to execute (required)")]
    command: String,
    #[arg(short, long, help = "Comma-separated list of node IDs to execute on")]
    nodes: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_str = std::fs::read_to_string("config.yaml")?;
    let config: Config = serde_yaml::from_str(&config_str)?;

    let target_node_ids: Option<HashSet<String>> = cli.nodes.as_ref().map(|s| {
        s.split(',').map(|id| id.trim().to_string()).collect()
    });

    let mut tasks = Vec::new();

    for node in config.nodes {
        if let Some(ref ids) = target_node_ids {
            if !ids.contains(&node.id) {
                continue;
            }
        }

        let login_script = config.login_script.clone();
        let command = cli.command.clone();

        tasks.push(tokio::spawn(async move {
            let commands = vec![myssh::ScriptStep {
                name: "cli".to_string(),
                wait: "$|#".to_string(),
                send: command,
            }];

            myssh::execute_ssh(
                node.host,
                node.port,
                node.user,
                node.password,
                login_script,
                commands,
            ).await
        }));
    }

    for task in tasks {
        if let Err(e) = task.await? {
            eprintln!("Task failed: {}", e);
        }
    }

    Ok(())
}
