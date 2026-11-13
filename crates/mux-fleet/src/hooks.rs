//! What an agent's hook was told, read as a report for the fleet.
//!
//! Claude Code, Codex and Copilot each run a command at the turns of a
//! session and hand it a JSON object on its standard input. The three
//! agree on most of it: Codex follows Claude Code's names, and Copilot
//! says the same things in camel case. So one reader takes all three,
//! going by the event's name and looking for each field under the names
//! any of them gives it. A field an agent stops sending, or a new event,
//! costs a detail rather than the report.

use std::path::PathBuf;

use serde_json::Value;

use crate::{AgentKind, AgentReport, Need, ReportEvent};

/// The report in a hook's input, if the event is one the fleet follows.
/// `event` is the name the hook was installed under; the input's own
/// `hook_event_name` stands in when it is left out.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn parse(kind: AgentKind, event: Option<&str>, input: &str, at_ms: u64) -> Option<AgentReport> {
    let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
    let name = event
        .map(str::to_owned)
        .or_else(|| text(&input, &["hook_event_name", "hookEventName"]))?;
    let event = match canonical(&name).as_str() {
        "sessionstart" => ReportEvent::SessionStarted {
            title: text(&input, &["session_title", "sessionTitle"]),
        },
        "userpromptsubmit" | "userpromptsubmitted" => {
            // A prompt the agent or a schedule wrote is work, not the
            // user's task.
            let authored = text(&input, &["source"]);
            match (text(&input, &["prompt"]), authored.as_deref()) {
                (Some(prompt), None | Some("user" | "sdk")) => ReportEvent::Prompted { prompt },
                _ => ReportEvent::Working { activity: None },
            }
        }
        "pretooluse" => {
            let tool = text(&input, &["tool_name", "toolName"]).unwrap_or_default();
            let arguments = tool_arguments(&input);
            match canonical(&tool).as_str() {
                "askuserquestion" | "askuser" | "requestuserinput" => ReportEvent::Blocked {
                    need: Need::Question,
                    message: question(&arguments),
                },
                "exitplanmode" => ReportEvent::Blocked {
                    need: Need::Question,
                    message: Some("Plan ready for review".to_owned()),
                },
                _ => ReportEvent::Working {
                    activity: Some(describe_tool(&tool, &arguments)),
                },
            }
        }
        "posttooluse" | "posttoolusefailure" => ReportEvent::Working { activity: None },
        "permissionrequest" => {
            let tool = text(&input, &["tool_name", "toolName"]).unwrap_or_default();
            ReportEvent::Blocked {
                need: Need::Permission,
                message: Some(describe_tool(&tool, &tool_arguments(&input)))
                    .filter(|message| !message.is_empty()),
            }
        }
        "notification" => {
            let message = text(&input, &["message"]);
            match text(&input, &["notification_type", "notificationType"]).as_deref() {
                Some("permission_prompt") => ReportEvent::Blocked {
                    need: Need::Permission,
                    message,
                },
                Some("elicitation_dialog") => ReportEvent::Blocked {
                    need: Need::Question,
                    message,
                },
                // Saying it has sat idle a while, or that a login worked,
                // changes nothing the fleet shows.
                _ => return None,
            }
        }
        "elicitation" => ReportEvent::Blocked {
            need: Need::Question,
            message: text(&input, &["message"]),
        },
        "stop" | "agentstop" => {
            let waiting = input
                .get("background_tasks")
                .and_then(Value::as_array)
                .is_some_and(|tasks| !tasks.is_empty());
            if waiting {
                ReportEvent::Working {
                    activity: Some("Waiting on background tasks".to_owned()),
                }
            } else {
                ReportEvent::Finished {
                    message: text(&input, &["last_assistant_message", "lastAssistantMessage"]),
                }
            }
        }
        "stopfailure" | "erroroccurred" => ReportEvent::Failed {
            message: text(&input, &["error_details", "errorDetails"])
                .or_else(|| text(&input, &["error", "message"]))
                .or_else(|| {
                    input
                        .get("error")
                        .and_then(|error| text(error, &["message", "name"]))
                }),
        },
        "sessionend" => {
            // Clearing a session starts another in the same agent.
            if text(&input, &["reason"]).as_deref() == Some("clear") {
                return None;
            }
            ReportEvent::SessionEnded
        }
        _ => return None,
    };
    Some(AgentReport {
        kind,
        event,
        agent_session: text(&input, &["session_id", "sessionId"]),
        transcript: text(&input, &["transcript_path", "transcriptPath"]).map(PathBuf::from),
        cwd: text(&input, &["cwd"])
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_absolute()),
        at_ms,
    })
}

/// An event's or a tool's name without its case or its separators, so
/// `PreToolUse`, `preToolUse` and `pre_tool_use` are one.
fn canonical(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The first of `keys` that holds text.
fn text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(key)?.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A tool's arguments. Copilot sends them as JSON in a string.
fn tool_arguments(input: &Value) -> Value {
    let arguments = ["tool_input", "toolInput", "toolArgs", "tool_args"]
        .iter()
        .find_map(|key| input.get(key))
        .cloned()
        .unwrap_or(Value::Null);
    match arguments {
        Value::String(encoded) => serde_json::from_str(&encoded).unwrap_or(Value::String(encoded)),
        other => other,
    }
}

/// What the agent asks, from a question tool's arguments.
fn question(arguments: &Value) -> Option<String> {
    text(arguments, &["question", "prompt", "message"]).or_else(|| {
        arguments
            .get("questions")?
            .as_array()?
            .iter()
            .find_map(|question| text(question, &["question", "header"]))
    })
}

/// A tool call in a few words: the tool and the one argument that says
/// what it is about, such as `Bash: cargo test` or `Edit: src/lib.rs`.
#[must_use]
pub fn describe_tool(tool: &str, arguments: &Value) -> String {
    // `mcp__server__tool` reads better as `server: tool`.
    if let Some(rest) = tool.strip_prefix("mcp__")
        && let Some((server, name)) = rest.split_once("__")
    {
        return format!("{server}: {name}");
    }
    let detail = if let Value::String(only) = arguments {
        Some(only.clone())
    } else {
        const TELLING: [&str; 14] = [
            "command",
            "file_path",
            "filePath",
            "path",
            "notebook_path",
            "pattern",
            "query",
            "url",
            "description",
            "subject",
            "skill",
            "prompt",
            "cmd",
            "name",
        ];
        TELLING.iter().find_map(|key| match arguments.get(key)? {
            Value::String(value) => Some(value.clone()),
            // Codex gives a command as its arguments.
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
    };
    let detail = detail
        .as_deref()
        .and_then(|detail| detail.lines().find(|line| !line.trim().is_empty()))
        .map(str::trim)
        .filter(|detail| !detail.is_empty());
    match (tool.trim(), detail) {
        ("", Some(detail)) => detail.to_owned(),
        (tool, Some(detail)) => format!("{tool}: {detail}"),
        (tool, None) => tool.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: AgentKind, name: &str, input: &str) -> Option<ReportEvent> {
        parse(kind, Some(name), input, 7).map(|report| report.event)
    }

    #[test]
    fn claude_code_says_who_it_is_and_what_was_asked() {
        let report = parse(
            AgentKind::Claude,
            None,
            r#"{"session_id":"abc-1","transcript_path":"/Users/r/.claude/projects/x/abc-1.jsonl",
                "cwd":"/Users/r/projects/mux","hook_event_name":"UserPromptSubmit",
                "prompt":"fix the flaky test","source":"user"}"#,
            7,
        )
        .expect("report");
        assert_eq!(report.kind, AgentKind::Claude);
        assert_eq!(report.agent_session.as_deref(), Some("abc-1"));
        assert_eq!(
            report.cwd.as_deref(),
            Some(std::path::Path::new("/Users/r/projects/mux"))
        );
        assert_eq!(
            report.transcript.as_deref(),
            Some(std::path::Path::new(
                "/Users/r/.claude/projects/x/abc-1.jsonl"
            ))
        );
        assert_eq!(report.at_ms, 7);
        assert_eq!(
            report.event,
            ReportEvent::Prompted {
                prompt: "fix the flaky test".to_owned()
            }
        );
        // A wake-up the agent set for itself is work, not a new task.
        assert_eq!(
            event(
                AgentKind::Claude,
                "UserPromptSubmit",
                r#"{"prompt":"check the build","source":"schedule_wakeup"}"#
            ),
            Some(ReportEvent::Working { activity: None })
        );
    }

    #[test]
    fn tools_are_named_with_what_they_are_about() {
        assert_eq!(
            event(
                AgentKind::Claude,
                "PreToolUse",
                r#"{"tool_name":"Bash","tool_input":{"command":"cargo test -p mux-fleet\necho done","description":"Run tests"}}"#
            ),
            Some(ReportEvent::Working {
                activity: Some("Bash: cargo test -p mux-fleet".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "PreToolUse",
                r#"{"tool_name":"mcp__github__create_pull_request","tool_input":{"title":"x"}}"#
            ),
            Some(ReportEvent::Working {
                activity: Some("github: create_pull_request".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Codex,
                "PreToolUse",
                r#"{"tool_name":"shell","tool_input":{"command":["bash","-lc","ls"]}}"#
            ),
            Some(ReportEvent::Working {
                activity: Some("shell: bash -lc ls".to_owned())
            })
        );
        // Copilot: camel case, with the arguments as JSON in a string.
        assert_eq!(
            event(
                AgentKind::Copilot,
                "preToolUse",
                r#"{"sessionId":"s1","toolName":"edit","toolArgs":"{\"path\":\"src/main.rs\"}"}"#
            ),
            Some(ReportEvent::Working {
                activity: Some("edit: src/main.rs".to_owned())
            })
        );
        assert_eq!(describe_tool("Read", &Value::Null), "Read");
    }

    #[test]
    fn asking_the_user_is_waiting_on_them() {
        assert_eq!(
            event(
                AgentKind::Claude,
                "PermissionRequest",
                r#"{"tool_name":"Write","tool_input":{"file_path":"src/foo.rs","content":"…"}}"#
            ),
            Some(ReportEvent::Blocked {
                need: Need::Permission,
                message: Some("Write: src/foo.rs".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "PreToolUse",
                r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which database?","header":"DB"}]}}"#
            ),
            Some(ReportEvent::Blocked {
                need: Need::Question,
                message: Some("Which database?".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "Notification",
                r#"{"message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#
            ),
            Some(ReportEvent::Blocked {
                need: Need::Permission,
                message: Some("Claude needs your permission to use Bash".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "Notification",
                r#"{"message":"Claude is waiting for your input","notification_type":"idle_prompt"}"#
            ),
            None
        );
    }

    #[test]
    fn a_turn_ends_done_unless_work_runs_on_behind_it() {
        assert_eq!(
            event(
                AgentKind::Codex,
                "Stop",
                r#"{"turn_id":"t1","stop_hook_active":false,"last_assistant_message":"All green."}"#
            ),
            Some(ReportEvent::Finished {
                message: Some("All green.".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Copilot,
                "agentStop",
                r#"{"stopReason":"end_turn"}"#
            ),
            Some(ReportEvent::Finished { message: None })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "Stop",
                r#"{"background_tasks":[{"id":"1"}],"last_assistant_message":"Waiting."}"#
            ),
            Some(ReportEvent::Working {
                activity: Some("Waiting on background tasks".to_owned())
            })
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "StopFailure",
                r#"{"error":"rate_limit","error_details":"Usage limit reached"}"#
            ),
            Some(ReportEvent::Failed {
                message: Some("Usage limit reached".to_owned())
            })
        );
    }

    #[test]
    fn sessions_start_and_end_but_clearing_one_is_neither() {
        assert_eq!(
            event(
                AgentKind::Claude,
                "SessionStart",
                r#"{"source":"resume","session_title":"Flaky test"}"#
            ),
            Some(ReportEvent::SessionStarted {
                title: Some("Flaky test".to_owned())
            })
        );
        assert_eq!(
            event(AgentKind::Claude, "SessionEnd", r#"{"reason":"clear"}"#),
            None
        );
        assert_eq!(
            event(
                AgentKind::Claude,
                "SessionEnd",
                r#"{"reason":"prompt_input_exit"}"#
            ),
            Some(ReportEvent::SessionEnded)
        );
    }

    #[test]
    fn input_that_is_not_json_still_reports_the_event() {
        assert_eq!(
            event(AgentKind::Copilot, "sessionStart", ""),
            Some(ReportEvent::SessionStarted { title: None })
        );
        assert_eq!(event(AgentKind::Claude, "PreCompact", "{}"), None);
        assert_eq!(parse(AgentKind::Claude, None, "{}", 0), None);
    }
}
