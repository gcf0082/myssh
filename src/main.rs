use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;

#[derive(Debug, serde::Deserialize)]
struct SshverConfig {
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
    #[arg(short, long, default_value = "false", help = "Interactive mode (keep connection open for multiple commands)")]
    interactive: bool,
    #[arg(long, help = "Parallel execute but print each node's output as one grouped block in node order")]
    sync: bool,
}

struct InteractiveSession {
    cli: Cli,
    ssh_config: SshverConfig,
    target_node_ids: Option<HashSet<String>>,
    current_dir: Option<String>,
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
    ssh_config: &SshverConfig,
    command: &str,
    target_node_ids: &Option<HashSet<String>>,
    current_dir: &Option<String>,
) -> Result<bool> {
    let mut tasks = Vec::new();

    for node in &ssh_config.nodes {
        if let Some(ref ids) = target_node_ids {
            if !ids.contains(&node.id) {
                continue;
            }
        }

        let node_password = if node.password.is_empty() {
            ssh_config.defaults.password.clone()
        } else {
            node.password.clone()
        };

        let node_user = if node.user.is_empty() {
            ssh_config.defaults.user.clone()
        } else {
            node.user.clone()
        };

        let node_port = if node.port == 22 && ssh_config.defaults.port != 22 {
            ssh_config.defaults.port
        } else {
            node.port
        };

        let login_script = build_login_script(&ssh_config.defaults, node, &node_password);
        let use_jump = node.use_jump.or(Some(ssh_config.defaults.use_jump)).unwrap_or(false);
        let command_wait = ssh_config.defaults.command_wait.clone();

        let jump_host = ssh_config.jump.host.clone();
        let jump_port = ssh_config.jump.port;
        let jump_user = ssh_config.jump.user.clone();
        let jump_password = ssh_config.jump.password.clone();
        let jump_login_script = ssh_config.jump.login_script.clone();

        let node_id = node.id.clone();
        let node_host = node.host.clone();
        let command = command.to_string();

        let verbose = cli.verbose;
        let prefix = cli.prefix;
        let sync = cli.sync;
        let current_dir = current_dir.clone();

        tasks.push(tokio::spawn(async move {
            let actual_command = if let Some(ref dir) = current_dir {
                format!("cd {} && {}", dir, command)
            } else {
                command
            };

            let commands = vec![myssh::ScriptStep {
                name: "cli".to_string(),
                wait: command_wait,
                send: actual_command,
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
                    sync,
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
                    sync,
                ).await
            }
        }));
    }

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    // spawn 顺序已经 = 配置/--nodes 过滤后的节点顺序，
    // 串行 await 每个 handle 并 replay 其捕获的输出即天然按节点顺序打印（sync 模式）。
    // 非 sync 模式下捕获 vec 恒为空，println 循环等价空操作。
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
                    Ok(Ok((success, lines))) => {
                        for line in &lines {
                            println!("{}", line);
                        }
                        if !success { failed = true; }
                    }
                    Ok(Err(e)) => eprintln!("\nTask failed: {}", e),
                    Err(_) => {}
                }
            }
            failed
        } => result
    };

    Ok(any_failed)
}
fn parse_special_command(line: &str) -> Option<(&str, &str)> {
    if let Some(pos) = line.find("!!") {
        let rest = line[pos + 2..].trim_start();
        if let Some(idx) = rest.find(' ') {
            Some((&rest[..idx], rest[idx + 1..].trim()))
        } else if rest.is_empty() {
            None
        } else {
            Some((rest, ""))
        }
    } else {
        None
    }
}

fn get_node_names(ssh_config: &SshverConfig, target_node_ids: &Option<HashSet<String>>) -> String {
    if let Some(ref ids) = target_node_ids {
        let mut names: Vec<_> = ssh_config.nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .map(|n| n.id.clone())
            .collect();
        names.sort();
        names.join(",")
    } else {
        "all".to_string()
    }
}

fn handle_special_command(
    session: &mut InteractiveSession,
    cmd: &str,
    args: &str,
) -> Result<bool> {
    match cmd {
        "help" => {
            println!("Special commands (!! prefix):");
            println!("  !!help                 - Show this help");
            println!("  !!node <subcmd>        - Node management commands");
            println!("      set <list|all>    - Set target nodes (e.g., !!node set node1,node2)");
            println!("      list               - List all node IDs");
            println!("      list -v            - List all nodes with details");
            println!("  !!cd <path>            - Change working directory");
            println!("  !!pwd                  - Show current working directory");
            println!("  !!prefix [on|off]      - Toggle per-line [node] prefix (no arg shows state)");
            println!("  !!sync   [on|off]      - Toggle grouped-per-node output (no arg shows state)");
            println!("                           Note: do NOT enable --sync for streaming commands");
            println!("                                 like tail -f / ping — output buffers until exit.");
        }
        "node" => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.is_empty() {
                eprintln!("Error: Missing subcommand. Use '!!node help' for usage.");
                return Ok(false);
            }

            let subcmd = parts[0];
            let subargs = if parts.len() > 1 { &parts[1..] } else { &[] as &[&str] };

            match subcmd {
                "set" => {
                    if subargs.is_empty() {
                        session.target_node_ids = None;
                        println!("Target nodes: all");
                    } else {
                        let nodes_str = subargs.join(" ");
                        let new_ids: HashSet<String> = nodes_str.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();

                        let all_node_ids: HashSet<String> = session.ssh_config.nodes
                            .iter()
                            .map(|n| n.id.clone())
                            .collect();

                        let missing: Vec<_> = new_ids.iter()
                            .filter(|id| !all_node_ids.contains(*id))
                            .map(|s| s.to_string())
                            .collect();

                        if !missing.is_empty() {
                            eprintln!("Error: Node(s) not found: {}", missing.join(", "));
                            return Ok(false);
                        }

                        session.target_node_ids = Some(new_ids);
                        println!("Target nodes: {}", nodes_str);
                    }
                }
                "list" => {
                    let verbose = subargs.iter().any(|&s| s == "-v");
                    if verbose {
                        println!("Node details:");
                        for node in &session.ssh_config.nodes {
                            println!("  {} - {}", node.id, node.host);
                        }
                    } else {
                        let mut node_ids: Vec<_> = session.ssh_config.nodes.iter().map(|n| n.id.clone()).collect();
                        node_ids.sort();
                        println!("Available nodes: {}", node_ids.join(", "));
                    }
                }
                "help" | _ => {
                    println!("Node management commands:");
                    println!("  !!node set <list|all> - Set target nodes");
                    println!("  !!node list            - List all node IDs");
                    println!("  !!node list -v         - List all nodes with details");
                }
            }
        }
        "cd" => {
            let new_dir = if args.starts_with('/') {
                args.to_string()
            } else if let Some(ref dir) = session.current_dir {
                format!("{}/{}", dir, args)
            } else {
                format!("/{}", args)
            };
            session.current_dir = Some(new_dir.clone());
            println!("Working directory: {}", new_dir);
        }
        "pwd" => {
            if let Some(ref dir) = session.current_dir {
                println!("Current working directory: {}", dir);
            } else {
                println!("Current working directory: (not set)");
            }
        }
        "prefix" => match args.trim() {
            "" => println!("Prefix: {}", if session.cli.prefix { "on" } else { "off" }),
            "on" => {
                session.cli.prefix = true;
                println!("Prefix: on");
            }
            "off" => {
                session.cli.prefix = false;
                println!("Prefix: off");
            }
            _ => {
                eprintln!("Usage: !!prefix [on|off]");
                return Ok(false);
            }
        },
        "sync" => match args.trim() {
            "" => println!("Sync: {}", if session.cli.sync { "on" } else { "off" }),
            "on" => {
                session.cli.sync = true;
                println!("Sync: on  (reminder: do not use with streaming commands like tail -f)");
            }
            "off" => {
                session.cli.sync = false;
                println!("Sync: off");
            }
            _ => {
                eprintln!("Usage: !!sync [on|off]");
                return Ok(false);
            }
        },
        _ => {
            eprintln!("Unknown command: !!{}", cmd);
            eprintln!("Type '!!help' for available commands");
            return Ok(false);
        }
    }
    Ok(true)
}

async fn run_interactive_session() -> Result<()> {
    use rustyline::error::ReadlineError;
    use rustyline::history::MemHistory;

    let cli = Cli::parse();
    let config_str = std::fs::read_to_string("config.yaml")?;
    let ssh_config: SshverConfig = serde_yaml::from_str(&config_str)?;

    let target_node_ids: Option<HashSet<String>> = cli.nodes.as_ref().map(|s| {
        s.split(',').map(|id| id.trim().to_string()).collect()
    });

    if let Some(ref ids) = target_node_ids {
        let all_node_ids: HashSet<String> = ssh_config.nodes.iter().map(|n| n.id.clone()).collect();
        let missing_ids: Vec<String> = ids.iter().filter(|id| !all_node_ids.contains(*id)).cloned().collect();
        if !missing_ids.is_empty() {
            eprintln!("Error: Node(s) not found: {}", missing_ids.join(", "));
            std::process::exit(1);
        }
    }

    let mut session = InteractiveSession {
        cli,
        ssh_config,
        target_node_ids,
        current_dir: None,
    };

    let node_names = get_node_names(&session.ssh_config, &session.target_node_ids);
    println!("Interactive mode. Type 'exit' or 'quit' to leave, '!!help' for commands.");
    println!("Connected to {} node(s).", node_names);

    let h = MemHistory::new();
    let mut editor = rustyline::Editor::<(), MemHistory>::with_history(rustyline::Config::default(), h)?;

    loop {
        let node_names = get_node_names(&session.ssh_config, &session.target_node_ids);
        let dir_info = session.current_dir.as_ref().map_or(String::new(), |d| format!("{}", d));

        let prompt = if dir_info.is_empty() {
            format!("myssh[{}]> ", node_names)
        } else {
            format!("myssh[{}]{}> ", node_names, dir_info)
        };

        let readline: rustyline::Result<String> = editor.readline(&prompt);

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

                if let Some((cmd, args)) = parse_special_command(trimmed) {
                    handle_special_command(&mut session, cmd, args)?;
                } else {
                    let failed = execute_on_all_nodes(
                        &session.cli,
                        &session.ssh_config,
                        trimmed,
                        &session.target_node_ids,
                        &session.current_dir,
                    ).await?;
                    if failed {
                        eprintln!("\nSome commands failed.");
                    }
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
    let config: SshverConfig = serde_yaml::from_str(&config_str)?;

    if cli.interactive {
        run_interactive_session().await?;
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

        let any_failed = execute_on_all_nodes(&cli, &config, command, &target_node_ids, &None).await?;

        if any_failed {
            std::process::exit(1);
        }
    }

    Ok(())
}
