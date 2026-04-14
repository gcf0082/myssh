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
    #[arg(short, long, help = "Command to execute (required in non-interactive mode)")]
    command: Option<String>,
    #[arg(short, long, help = "Comma-separated list of node IDs to execute on")]
    nodes: Option<String>,
    #[arg(short, long, help = "Show debug information")]
    verbose: bool,
    #[arg(long, help = "Add node prefix to each output line")]
    prefix: bool,
    #[arg(long, default_value = "false", help = "Interactive mode (keep connection open for multiple commands)")]
    interactive: bool,
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

async fn execute_on_all_nodes(
    cli: &Cli,
    config: &Config,
    command: &str,
    target_node_ids: &Option<HashSet<String>>,
) -> Result<bool> {
    let mut tasks = Vec::new();

    for node in &config.nodes {
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

        let login_script = build_login_script(&config.defaults, node, &node_password);
        let use_jump = node.use_jump.or(Some(config.defaults.use_jump)).unwrap_or(false);
        let command_wait = config.defaults.command_wait.clone();

        let jump_host = config.jump.host.clone();
        let jump_port = config.jump.port;
        let jump_user = config.jump.user.clone();
        let jump_password = config.jump.password.clone();
        let jump_login_script = config.jump.login_script.clone();

        let node_id = node.id.clone();
        let node_host = node.host.clone();
        let command = command.to_string();

        let verbose = cli.verbose;
        let prefix = cli.prefix;

        tasks.push(tokio::spawn(async move {
            let commands = vec![myssh::ScriptStep {
                name: "cli".to_string(),
                wait: command_wait,
                send: command,
            }];

            if use_jump {
                myssh::execute_ssh_via_jump(
            node_id,
                    jump_host,
                    jump_port,
                    jump_user,
                    jump_password,
                    jump_login_script,
                    node_host,
                    node_port,
                    node_user,
                    node_password,
                    login_script,
                    commands,
                    verbose,
                    prefix,
                ).await
            } else {
                myssh::execute_ssh(
                    node_id,
                    node_host,
                    node_port,
                    node_user,
                    node_password,
                    login_script,
                    commands,
                    verbose,
                    prefix,
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

    Ok(any_failed)
}

async fn run_interactive_session(cli: Cli, config: Config) -> Result<()> {
    use rustyline::error::ReadlineError;
    use rustyline::history::MemHistory;
    use rustyline::Config;

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

    let node_count = if let Some(ref ids) = target_node_ids {
        config.nodes.iter().filter(|n| ids.contains(&n.id)).count()
    } else {
        config.nodes.len()
    };

    println!("Interactive mode. Type 'exit' or 'quit' to leave.");
    println!("Connected to {} node(s).", node_count);

    let h = MemHistory::new();
    let mut editor = rustyline::Editor::<(), MemHistory>::with_history(Config::default(), h)?;

    loop {
        let readline: rustyline::Result<String> = editor.readline("myssh> ");

        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "exit" || trimmed == "quit" {
                    println!("Exiting...");
                    break;
                }

                editor.add_history_entry(line.as_str())?;

                let failed = execute_on_all_nodes(&cli, &config, &line, &target_node_ids).await?;
                if failed {
                    eprintln!("Some commands failed.");
                }
            }
            Err(ReadlineError::Eof) => {
                println!("EOF received, exiting...");
                break;
            }
            Err(ReadlineError::Interrupted) => {
                println!("Ctrl-C received, use 'exit' to quit or continue typing.");
                continue;
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_str = std::fs::read_to_string("config.yaml")?;
    let config: Config = serde_yaml::from_str(&config_str)?;

    if cli.interactive {
        run_interactive_session(cli, config).await?;
    } else {
        let command = cli.command.as_ref().ok_or_else(|| anyhow::anyhow!("--command is required in non-interactive mode"))?;

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

        let any_failed = execute_on_all_nodes(&cli, &config, command, &target_node_ids).await?;

        if any_failed {
            std::process::exit(1);
        }
    }

    Ok(())
}
