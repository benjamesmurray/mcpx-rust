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
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
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

        run_tool(server_config, &server_name, cli.tool.as_deref(), cli.args, cli.json).await?;
    } else {
        println!("Usage: mcpx <server> [tool] [args...] or mcpx list");
    }

    Ok(())
}

fn get_cache_dir() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let cache_dir = std::path::PathBuf::from(home).join(".cache/mcpx-rust/schemas");
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)?;
    }
    Ok(cache_dir)
}

fn get_cached_tools(server_name: &str) -> Result<Option<ListToolsResult>> {
    let cache_file = get_cache_dir()?.join(format!("{}.json", server_name));
    if !cache_file.exists() {
        return Ok(None);
    }

    let metadata = std::fs::metadata(&cache_file)?;
    let modified = metadata.modified()?;
    let elapsed = modified.elapsed()?.as_secs();

    // Cache expires after 24 hours
    if elapsed > 24 * 3600 {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&cache_file)?;
    let tools: ListToolsResult = serde_json::from_str(&content)?;
    Ok(Some(tools))
}

fn save_tools_to_cache(server_name: &str, tools: &ListToolsResult) -> Result<()> {
    let cache_file = get_cache_dir()?.join(format!("{}.json", server_name));
    let content = serde_json::to_string_pretty(tools)?;
    std::fs::write(cache_file, content)?;
    Ok(())
}

fn is_flat_schema(schema: &serde_json::Value) -> bool {
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        if properties.contains_key("args") {
            if let Some(args_schema) = properties.get("args").and_then(|s| s.as_object()) {
                if args_schema.get("type").and_then(|t| t.as_str()) == Some("array") {
                    return true;
                }
            }
        }
    }
    false
}

fn coerce_value(value: &str, schema: &serde_json::Value, key: &str) -> serde_json::Value {
    let prop_schema = schema.get("properties").and_then(|p| p.get(key));
    if let Some(prop_schema) = prop_schema {
        let type_str = prop_schema.get("type").and_then(|t| t.as_str());
        match type_str {
            Some("boolean") => match value.to_lowercase().as_str() {
                "true" | "yes" | "1" => serde_json::Value::Bool(true),
                "false" | "no" | "0" => serde_json::Value::Bool(false),
                _ => serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string())),
            },
            Some("number") | Some("integer") => {
                if let Ok(n) = value.parse::<i64>() {
                    serde_json::json!(n)
                } else if let Ok(f) = value.parse::<f64>() {
                    serde_json::json!(f)
                } else {
                    serde_json::Value::String(value.to_string())
                }
            }
            _ => serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string())),
        }
    } else {
        serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string()))
    }
}

fn normalize_arguments(
    tool: &Tool,
    raw_args: &[String],
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let schema = &tool.input_schema;
    let schema_val = serde_json::to_value(schema)?;
    let mut map = serde_json::Map::new();

    let is_flat = is_flat_schema(&schema_val);

    if is_flat {
        let mut flat_args = Vec::new();
        let mut i = 0;
        while i < raw_args.len() {
            let arg = &raw_args[i];
            if arg.starts_with("--") {
                let key = arg.trim_start_matches('-');
                if let Some((k, v)) = key.split_once('=') {
                    flat_args.push(format!("{}={}", k, v));
                } else if i + 1 < raw_args.len() && !raw_args[i + 1].starts_with('-') {
                    flat_args.push(format!("{}={}", key, raw_args[i + 1]));
                    i += 1;
                } else {
                    flat_args.push(format!("{}=true", key));
                }
            } else {
                flat_args.push(arg.to_string());
            }
            i += 1;
        }
        map.insert("args".to_string(), serde_json::json!(flat_args));
    } else {
        let mut i = 0;
        while i < raw_args.len() {
            let arg = &raw_args[i];
            if arg.starts_with("--") {
                let key = arg.trim_start_matches('-');
                if let Some((k, v)) = key.split_once('=') {
                    map.insert(k.to_string(), coerce_value(v, &schema_val, k));
                } else if i + 1 < raw_args.len() && !raw_args[i + 1].starts_with('-') {
                    map.insert(key.to_string(), coerce_value(&raw_args[i + 1], &schema_val, key));
                    i += 1;
                } else {
                    map.insert(key.to_string(), coerce_value("true", &schema_val, key));
                }
            } else if let Some((k, v)) = arg.split_once('=') {
                map.insert(k.to_string(), coerce_value(v, &schema_val, k));
            } else {
                // Skip positional args for structured schema for now
            }
            i += 1;
        }
    }

    Ok(map)
}

async fn run_tool(
    server_config: config::McpServerConfig,
    server_name: &str,
    tool_name: Option<&str>,
    raw_args: Vec<String>,
    json_output: bool,
) -> Result<()> {
    let client = connect_to_server(&server_config).await?;

    if let Some(name) = tool_name {
        let tools = if let Some(cached) = get_cached_tools(server_name)? {
            if cached.tools.iter().any(|t| t.name == name) {
                cached
            } else {
                let tools = client
                    .request_tool_list(None)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to list tools: {}", e))?;
                save_tools_to_cache(server_name, &tools)?;
                tools
            }
        } else {
            let tools = client
                .request_tool_list(None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list tools: {}", e))?;
            save_tools_to_cache(server_name, &tools)?;
            tools
        };

        let tool = tools
            .tools
            .into_iter()
            .find(|t| t.name == name)
            .with_context(|| format!("Tool '{}' not found on server", name))?;

        let arguments = normalize_arguments(&tool, &raw_args)?;

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
        save_tools_to_cache(server_name, &tools)?;
        
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mock_tool(schema: serde_json::Value) -> Tool {
        let tool_json = json!({
            "name": "test_tool",
            "inputSchema": schema
        });
        serde_json::from_value(tool_json).unwrap()
    }

    #[test]
    fn test_normalize_structured() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "count": { "type": "integer" },
                "active": { "type": "boolean" }
            }
        });
        let tool = mock_tool(schema);

        let raw_args = vec![
            "--name".to_string(), "foo".to_string(),
            "--count".to_string(), "42".to_string(),
            "--active".to_string()
        ];
        let normalized = normalize_arguments(&tool, &raw_args).unwrap();

        assert_eq!(normalized.get("name").unwrap(), "foo");
        assert_eq!(normalized.get("count").unwrap(), 42);
        assert_eq!(normalized.get("active").unwrap(), true);
    }

    #[test]
    fn test_normalize_flat() {
        let schema = json!({
            "type": "object",
            "properties": {
                "args": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        });
        let tool = mock_tool(schema);

        let raw_args = vec![
            "--name".to_string(), "foo".to_string(),
            "positional".to_string(),
            "--recursive".to_string()
        ];
        let normalized = normalize_arguments(&tool, &raw_args).unwrap();

        let args = normalized.get("args").unwrap().as_array().unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "name=foo");
        assert_eq!(args[1], "positional");
        assert_eq!(args[2], "recursive=true");
    }

    #[test]
    fn test_mixed_syntax() {
        let schema = json!({
            "type": "object",
            "properties": {
                "key1": { "type": "string" },
                "key2": { "type": "string" }
            }
        });
        let tool = mock_tool(schema);

        let raw_args = vec![
            "--key1=val1".to_string(),
            "key2=val2".to_string()
        ];
        let normalized = normalize_arguments(&tool, &raw_args).unwrap();

        assert_eq!(normalized.get("key1").unwrap(), "val1");
        assert_eq!(normalized.get("key2").unwrap(), "val2");
    }
}

