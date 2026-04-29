# mcpx-rust

A Rust implementation of `mcpx`, a CLI utility that transforms Model Context Protocol (MCP) servers into composable command-line tools.

## Features

- **Server Discovery**: List all configured MCP servers.
- **Tool Discovery**: List available tools for a specific server.
- **Dynamic Invocation**: Map MCP tool schemas to CLI arguments.
- **JSON Output**: Optional JSON output for piping into tools like `jq`.
- **Configuration**: Simple TOML-based configuration.

## Installation

```bash
cargo install mcpx-rust
```

## Configuration

`mcpx-rust` reads server definitions from `~/.config/mcpx/config.toml`.

Example `config.toml`:

```toml
[mcp_servers.everything]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]

[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "your_token_here" }
```

You can also use the `MCPX_CONFIG` environment variable to point to a different configuration file.

## Usage

```bash
Usage: mcpx-rust [OPTIONS] [SERVER] [TOOL] [ARGS]... [COMMAND]

To list tools for a server, use: mcpx-rust <SERVER>
```

### List configured servers
```bash
mcpx-rust list
```

### List tools for a server (Discovery)
```bash
mcpx-rust <server_name>
# OR
mcpx-rust <server_name> --help
```

### Call a tool
```bash
mcpx-rust <server_name> <tool_name> [args...]
```
Arguments are passed as `key=value` pairs. Values are automatically parsed as JSON (numbers, booleans, etc.) if possible, otherwise they are treated as strings.

Example:
```bash
mcpx-rust everything echo message="Hello from Rust"
```

### JSON output
```bash
mcpx-rust everything echo message="Hello" --json
```

## License

MIT
