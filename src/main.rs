use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;

#[derive(Debug, serde::Deserialize)]
struct Config {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    pub nodes: Vec<NodeConfig>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct DefaultsConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub login_script: Vec<myssh::ScriptStep>,
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, serde::Deserialize, Clone)]
struct NodeConfig {
    pub id: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub login_script: Vec<myssh::ScriptStep>,
    #[serde(default)]
    pub login_script_append: Vec<myssh::ScriptStep>,
}

#[derive(Parser, Debug)]
#[command(name = "myssh")]
struct Cli {
    #[arg(short, long, help = "Command to execute (required)")]
    command: String,
    #[arg(short, long, help = "Comma-separated list of node IDs to execute on")]
    nodes: Option<String>,
    #[arg(short, long, help = "Show debug information")]
    verbose: bool,
}

fn build_login_script(
    defaults: &DefaultsConfig,
    node: &NodeConfig,
    node_password: &str,
) -> Vec<myssh::ScriptStep> {
    if !node.login_script.is_empty() {
        let mut script = node.login_script.clone();
        for step in &mut script {
            if step.send == "{{password}}" {
                step.send = node_password.to_string();
            }
        }
        return script;
    }

    let mut script = defaults.login_script.clone();
    for step in &mut script {
        if step.send == "{{password}}" {
            step.send = node_password.to_string();
        }
    }

    let mut appended = node.login_script_append.clone();
    script.append(&mut appended);

    script
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

        let node_password = if node.password.is_empty() {
            config.defaults.password.clone()
        } else {
            node.password.clone()
        };

        let node_user = if node.user.is_empty() {
            config.defaults.user.clone()
        } else {
            node.user.clone()
        };

        let node_port = if node.port == 22 && config.defaults.port != 22 {
            config.defaults.port
        } else {
            node.port
        };

        let login_script = build_login_script(&config.defaults, &node, &node_password);
        let command = cli.command.clone();

        tasks.push(tokio::spawn(async move {
            let commands = vec![myssh::ScriptStep {
                name: "cli".to_string(),
                wait: "$|#".to_string(),
                send: command,
            }];

            myssh::execute_ssh(
                node.host,
                node_port,
                node_user,
                node_password,
                login_script,
                commands,
                cli.verbose,
            ).await
        }));
    }

    let ctrl_c = tokio::signal::ctrl_c();
tokio::pin!(ctrl_c);

let any_failed = tokio::select! {
    _ = ctrl_c.as_mut() => {
        eprintln!("\nCtrl+C received, aborting all tasks...");
        for handle in tasks {
            let _ = handle.abort();
        }
        true
    }
    result = async {
        let mut failed = false;
        for handle in &mut tasks {
            match handle.await {
                Ok(Ok(success)) if !success => failed = true,
                Ok(Err(e)) => eprintln!("Task failed: {}", e),
                _ => {}
            }
        }
        failed
    } => result
};

if any_failed {
    std::process::exit(1);
}

Ok(())
}