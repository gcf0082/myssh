use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;

#[derive(Debug, serde::Deserialize)]
struct Config {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub jump: JumpConfig,
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
    #[serde(default)]
    pub use_jump: bool,
    #[serde(default = "default_command_wait")]
    pub command_wait: String,
}

#[derive(Debug, serde::Deserialize, Default)]
struct JumpConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub password: String,
    #[serde(default)]
    pub login_script: Vec<myssh::ScriptStep>,
}

fn default_port() -> u16 {
    22
}

fn default_command_wait() -> String {
    "$|#".to_string()
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
    #[serde(default)]
    pub use_jump: Option<bool>,
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
    #[arg(long, help = "Add node prefix to each output line")]
    prefix: bool,
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

    if let Some(ref ids) = target_node_ids {
        let all_node_ids: HashSet<String> = config.nodes.iter().map(|n| n.id.clone()).collect();
        let missing_ids: Vec<String> = ids.iter().filter(|id| !all_node_ids.contains(*id)).cloned().collect();
        if !missing_ids.is_empty() {
            eprintln!("Error: Node(s) not found: {}", missing_ids.join(", "));
            std::process::exit(1);
        }
    }

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
        let use_jump = node.use_jump.or(Some(config.defaults.use_jump)).unwrap_or(false);
        let command_wait = config.defaults.command_wait.clone();

        let jump_host = config.jump.host.clone();
        let jump_port = config.jump.port;
        let jump_user = config.jump.user.clone();
        let jump_password = config.jump.password.clone();
        let jump_login_script = config.jump.login_script.clone();

        tasks.push(tokio::spawn(async move {
            let commands = vec![myssh::ScriptStep {
                name: "cli".to_string(),
                wait: command_wait,
                send: command,
            }];

            if use_jump {
                myssh::execute_ssh_via_jump(
                    node.id.clone(),
                    jump_host,
                    jump_port,
                    jump_user,
                    jump_password,
                    jump_login_script,
                    node.host,
                    node_port,
                    node_user,
                    node_password,
                    login_script,
                    commands,
                    cli.verbose,
                    cli.prefix,
                ).await
            } else {
                myssh::execute_ssh(
                    node.id.clone(),
                    node.host,
                    node_port,
                    node_user,
                    node_password,
                    login_script,
                    commands,
                    cli.verbose,
                    cli.prefix,
                ).await
            }
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
