#![forbid(unsafe_code)]

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    while let Some(frame) = read_frame(&mut reader)? {
        let Some(id) = frame.get("id").cloned() else {
            continue;
        };
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fake-mcp-server", "version": "0.1.0" }
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo_remote",
                    "description": "Echoes text from the MCP fixture",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" }
                        },
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }]
            }),
            "tools/call" => {
                let text = params
                    .get("arguments")
                    .and_then(|arguments| arguments.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("echo: {text}")
                    }],
                    "structuredContent": {
                        "echoed": text
                    },
                    "isError": false
                })
            }
            _ => json!({}),
        };

        write_frame(
            &mut writer,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
        )?;
    }

    Ok(())
}

fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid content length: {error}"),
                )
            })?);
        }
    }

    let Some(content_length) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length",
        ));
    };

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {error}"))
    })?;
    Ok(Some(value))
}

fn write_frame(writer: &mut impl Write, payload: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(payload).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {error}"))
    })?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}
