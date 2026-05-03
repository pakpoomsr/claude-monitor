use serde_json::Value;

#[derive(Debug, Clone)]
pub enum ClaudeEvent {
    SessionStart {
        session_id: String,
        project: String,
    },
    Usage {
        input: u64,
        output: u64,
        cache: u64,
        model: String,
    },
    AssistantText {
        text: String,
    },
    ToolUseStart {
        tool: String,
        id: String,
    },
    ToolUseEnd {
        id: String,
    },
    UserMessage,
    Unknown,
}

/// Parse a single JSONL line into zero or more ClaudeEvents.
/// One line can yield multiple events (e.g. an assistant message with text +
/// a tool_use block + a usage block).
pub fn parse_jsonl_line(line: &str) -> Vec<ClaudeEvent> {
    let Ok(val) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let event_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "system" => {
            let session_id = string_field(&val, "session_id");
            let project = val
                .get("cwd")
                .or_else(|| val.get("project_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(ClaudeEvent::SessionStart { session_id, project });
        }

        "assistant" => {
            if let Some(message) = val.get("message") {
                if let Some(usage) = message.get("usage") {
                    let input = u64_field(usage, "input_tokens");
                    let output = u64_field(usage, "output_tokens");
                    let cache = u64_field(usage, "cache_read_input_tokens");
                    let model = string_field(message, "model");
                    if input > 0 || output > 0 || cache > 0 {
                        out.push(ClaudeEvent::Usage { input, output, cache, model });
                    }
                }

                if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                let text = string_field(block, "text");
                                if !text.is_empty() {
                                    out.push(ClaudeEvent::AssistantText { text });
                                }
                            }
                            "tool_use" => {
                                let tool = string_field(block, "name");
                                let id = string_field(block, "id");
                                out.push(ClaudeEvent::ToolUseStart { tool, id });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        "user" | "human" => {
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                let mut tool_result_seen = false;
                for block in content {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type == "tool_result" {
                        let id = string_field(block, "tool_use_id");
                        out.push(ClaudeEvent::ToolUseEnd { id });
                        tool_result_seen = true;
                    }
                }
                if !tool_result_seen {
                    out.push(ClaudeEvent::UserMessage);
                }
            } else {
                out.push(ClaudeEvent::UserMessage);
            }
        }

        _ => out.push(ClaudeEvent::Unknown),
    }

    out
}

fn string_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|s| s.as_str()).unwrap_or("").to_string()
}

fn u64_field(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|s| s.as_u64()).unwrap_or(0)
}
