mod config;

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{CommandFactory, Parser, Subcommand};
use rust_mcp_sdk::{
    mcp_client::{client_runtime, ClientHandler, McpClientOptions, ToMcpClientHandler},
    schema::*,
    task_store::InMemoryTaskStore,
    *,
};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    author,
    version,
    about,
    long_about = None,
    override_usage = "mcpx-rust [OPTIONS] [SERVER] [TOOL] [ARGS]... [COMMAND]\n\nTo list tools for a server, use: mcpx-rust <SERVER>"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// The name of the server to use
    server: Option<String>,

    /// The tool to call
    tool: Option<String>,

    /// Output the result as JSON
    #[arg(long)]
    json: bool,

    /// Arguments for the tool (key=value)
    args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// List configured servers
    List,
}

pub struct MyClientHandler;
#[async_trait]
impl ClientHandler for MyClientHandler {}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let mut schema_mode = false;

    let has_help = args.iter().any(|arg| arg == "--help" || arg == "-h");
    let has_json = args.iter().any(|arg| arg == "--json");

    if has_help && has_json {
        schema_mode = true;
        args.retain(|arg| arg != "--help" && arg != "-h");
    }

    // Help Forwarding for Servers
    if !schema_mode {
        let has_help_flag_or_cmd = args.iter().any(|arg| arg == "--help" || arg == "-h" || arg == "help");
        if has_help_flag_or_cmd {
            let positionals: Vec<_> = args.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
            let non_help_positionals: Vec<_> = positionals.iter().filter(|&&a| a != "help").collect();

            // If exactly one positional (the server) and it's not the "list" command
            if non_help_positionals.len() == 1 && non_help_positionals[0].as_str() != "list" {
                args.retain(|arg| arg != "--help" && arg != "-h" && arg != "help");
            }
        }
    }

    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => {
            if schema_mode {
                Cli::command().print_help()?;
                return Ok(());
            }
            e.exit();
        }
    };

    let config = config::load_config().context("Failed to load configuration")?;

    match cli.command {
        Some(Commands::List) => {
            println!("Configured MCP Servers:");
            for name in config.mcp_servers.keys() {
                println!("  - {}", name);
            }
            return Ok(());
        }
        None => {}
    }

    if let Some(server_name) = cli.server {
        let server_config = config
            .mcp_servers
            .get(&server_name)
            .with_context(|| format!("Server '{}' not found in config", server_name))?
            .clone();

        if schema_mode {
            if let Some(tool_name) = cli.tool {
                extract_schema(server_config, &tool_name).await?;
                return Ok(());
            } else {
                Cli::command().print_help()?;
                return Ok(());
            }
        }

        // Parse key=value arguments
        let mut tool_args = serde_json::Map::new();
        for arg in cli.args {
            if let Some((key, value)) = arg.split_once('=') {
                // Try to parse value as JSON (number, bool, etc.), otherwise treat as string
                let json_value = serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string()));
                tool_args.insert(key.to_string(), json_value);
            } else {
                anyhow::bail!("Invalid argument format: '{}'. Expected key=value", arg);
            }
        }

        run_tool(server_config, cli.tool.as_deref(), tool_args, cli.json).await?;
    } else {
        println!("Usage: mcpx <server> [tool] [args...] or mcpx list");
    }

    Ok(())
}

async fn run_tool(
    server_config: config::McpServerConfig,
    tool_name: Option<&str>,
    arguments: serde_json::Map<String, serde_json::Value>,
    json_output: bool,
) -> Result<()> {
    let client = connect_to_server(&server_config).await?;

    if let Some(name) = tool_name {
        let request = CallToolRequestParams {
            name: name.to_string(),
            arguments: Some(arguments),
            meta: None,
            task: None,
        };

        let result = client
            .request_tool_call(request)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call tool: {}", e))?;

        if json_output {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            for content in result.content {
                match content {
                    ContentBlock::TextContent(text) => {
                        println!("{}", text.text);
                    }
                    ContentBlock::ImageContent(image) => {
                        println!("[Image: {}]", image.mime_type);
                    }
                    _ => {
                        println!("{:?}", content);
                    }
                }
            }
        }

        if result.is_error.unwrap_or(false) {
            std::process::exit(1);
        }
    } else {
        let tools = client
            .request_tool_list(None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list tools: {}", e))?;
        
        println!("Available tools:");
        for tool in tools.tools {
            println!("  - {}: {}", tool.name, tool.description.unwrap_or_default());
        }
    }

    client.shut_down().await.map_err(|e| anyhow::anyhow!("Failed to shut down client: {}", e))?;

    Ok(())
}

async fn extract_schema(
    server_config: config::McpServerConfig,
    tool_name: &str,
) -> Result<()> {
    let client = connect_to_server(&server_config).await?;

    let tools = client
        .request_tool_list(None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list tools: {}", e))?;

    let tool = tools.tools.into_iter().find(|t| t.name == tool_name)
        .with_context(|| format!("Tool '{}' not found on server", tool_name))?;

    let output = serde_json::json!({
        "input_schema": tool.input_schema
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    client.shut_down().await.map_err(|e| anyhow::anyhow!("Failed to shut down client: {}", e))?;

    Ok(())
}

async fn connect_to_server(
    server_config: &config::McpServerConfig,
) -> Result<Arc<mcp_client::ClientRuntime>> {
    let client_details = InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "mcpx-rust".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: None,
            icons: vec![],
            title: None,
            website_url: None,
        },
        protocol_version: LATEST_PROTOCOL_VERSION.into(),
        meta: None,
    };

    let transport = StdioTransport::create_with_server_launch(
        &server_config.command,
        server_config.args.clone(),
        server_config.env.clone(),
        TransportOptions::default(),
    ).map_err(|e| anyhow::anyhow!("Failed to create transport: {}", e))?;

    let handler = MyClientHandler {};

    let client = client_runtime::create_client(McpClientOptions {
        client_details,
        transport,
        handler: handler.to_mcp_client_handler(),
        task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
        server_task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
        message_observer: None,
    });

    client.clone().start().await.map_err(|e| anyhow::anyhow!("Failed to start client: {}", e))?;

    Ok(client)
}
