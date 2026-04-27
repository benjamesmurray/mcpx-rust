use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::{Result, Context};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
}

pub fn load_config() -> Result<Config> {
    let config_path = if let Ok(path) = std::env::var("MCPX_CONFIG") {
        PathBuf::from(path)
    } else {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        PathBuf::from(home).join(".config/mcpx/config.toml")
    };
    
    if !config_path.exists() {
        return Ok(Config {
            mcp_servers: HashMap::new(),
        });
    }

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file at {:?}", config_path))?;
    
    let config: Config = toml::from_str(&config_str)
        .with_context(|| format!("Failed to parse TOML config at {:?}", config_path))?;
    
    Ok(config)
}
