//! Local stdio Model Context Protocol (MCP) server for ScreenCraft.

use screencraft_mcp::McpServer;
use std::io::{BufRead, Write};

fn main() {
    let mut server = McpServer::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_message(trimmed) {
            if let Ok(text) = serde_json::to_string(&resp) {
                if writeln!(out, "{text}").is_err() || out.flush().is_err() {
                    break;
                }
            }
        }
    }
}
