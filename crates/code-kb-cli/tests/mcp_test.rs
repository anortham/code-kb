use std::io::Write;
use std::process::{Command, Stdio};
use serde_json::{json, Value};

#[test]
fn test_mcp_stdio_handshake_and_tools() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn code-kb serve");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);

    // 1. Send initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-agent",
                "version": "1.0"
            }
        }
    });

    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut response_line).unwrap();

    let resp: Value = serde_json::from_str(&response_line).expect("Failed to parse JSON response");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "code-kb");

    // 2. Send tools/list
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let mut line2 = serde_json::to_string(&list_req).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line2 = String::new();
    reader.read_line(&mut response_line2).unwrap();

    let resp2: Value = serde_json::from_str(&response_line2).expect("Failed to parse JSON response");
    assert_eq!(resp2["id"], 2);
    let tools = resp2["result"]["tools"].as_array().expect("Expected tools array");
    assert_eq!(tools.len(), 8);

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    assert!(tool_names.contains(&"codebase_outline"));
    assert!(tool_names.contains(&"file_skeleton"));
    assert!(tool_names.contains(&"find_symbol"));
    assert!(tool_names.contains(&"get_symbol_body"));
    assert!(tool_names.contains(&"get_context_slice"));
    assert!(tool_names.contains(&"find_references"));
    assert!(tool_names.contains(&"find_structural_facts"));
    assert!(tool_names.contains(&"replace_symbol_body"));

    // Verify zero workspace pollution across all tools
    for tool in tools {
        let schema = &tool["inputSchema"];
        let props = &schema["properties"];
        assert!(
            props.get("workspace").is_none(),
            "Tool '{}' should NOT expose 'workspace' parameter in schema",
            tool["name"]
        );
    }

    // 3. Send tools/call find_symbol
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "find_symbol",
            "arguments": {
                "query": "Workspace",
                "kind": "struct"
            }
        }
    });

    let mut line3 = serde_json::to_string(&call_req).unwrap();
    line3.push('\n');
    stdin.write_all(line3.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line3 = String::new();
    reader.read_line(&mut response_line3).unwrap();

    let resp3: Value = serde_json::from_str(&response_line3).expect("Failed to parse JSON response");
    assert_eq!(resp3["id"], 3);
    let content_text = resp3["result"]["content"][0]["text"].as_str().unwrap();
    eprintln!("content_text: {content_text}");
    assert!(content_text.contains("Workspace"));

    drop(stdin);
    let _ = child.wait();
}
