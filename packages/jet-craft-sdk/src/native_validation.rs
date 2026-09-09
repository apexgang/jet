//! Supported native configuration shapes, checked before touching user settings.
use serde_json::Value;

pub(crate) fn valid(
	harness: &str,
	kind: &str,
	name: &str,
	value: &Value,
) -> bool {
	if kind == "mcp" {
		return mcp(harness, value);
	}
	if ![
		"SessionStart",
		"SessionEnd",
		"UserPromptSubmit",
		"PreToolUse",
		"PermissionRequest",
		"PostToolUse",
		"PostToolUseFailure",
		"Notification",
		"SubagentStart",
		"SubagentStop",
		"Stop",
		"StopFailure",
		"TeammateIdle",
		"TaskCompleted",
		"InstructionsLoaded",
		"ConfigChange",
		"WorktreeCreate",
		"WorktreeRemove",
		"PreCompact",
		"PostCompact",
		"Elicitation",
		"ElicitationResult",
	]
	.contains(&name)
	{
		return false;
	}
	value.as_array().is_some_and(|groups| {
		!groups.is_empty()
			&& groups.iter().all(|group| {
				group.as_object().is_some_and(|g| {
					g.iter().all(|(key, value)| match key.as_str() {
						"matcher" => value.is_string(),
						"hooks" => value.as_array().is_some_and(|hooks| {
							!hooks.is_empty() && hooks.iter().all(hook)
						}),
						_ => false,
					}) && g.contains_key("hooks")
				})
			})
	})
}
fn mcp(harness: &str, value: &Value) -> bool {
	let Some(fields) = value.as_object() else {
		return false;
	};
	let command = fields
		.get("command")
		.is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
	let url = fields.get("url").is_some_and(|v| {
		v.as_str().is_some_and(|s| {
			s.starts_with("https://") || s.starts_with("http://")
		})
	});
	let transport = match fields.get("type").and_then(Value::as_str) {
		Some("stdio") => command,
		Some("http" | "sse") => url,
		None => true,
		Some(_) => false,
	};
	let selected = if fields.contains_key("command") {
		command && !fields.contains_key("url")
	} else {
		url && !fields.contains_key("args") && !fields.contains_key("env")
	};
	selected
		&& transport
		&& fields.iter().all(|(key, value)| {
			let allowed = if harness == "codex" {
				match key.as_str() {
					"command" | "args" | "env" | "env_vars" | "cwd" => command,
					"url"
					| "bearer_token_env_var"
					| "http_headers"
					| "env_http_headers" => url,
					"enabled"
					| "required"
					| "startup_timeout_ms"
					| "startup_timeout_sec"
					| "tool_timeout_sec"
					| "enabled_tools"
					| "disabled_tools" => true,
					_ => false,
				}
			} else {
				match key.as_str() {
					"command" | "args" | "env" => command,
					"url" | "headers" => url,
					"type" => true,
					_ => false,
				}
			};
			allowed
				&& match key.as_str() {
					"command"
					| "url"
					| "cwd"
					| "bearer_token_env_var"
					| "type" => value.is_string(),
					"args" | "env_vars" | "enabled_tools"
					| "disabled_tools" => value.as_array().is_some_and(|values| {
						values.iter().all(Value::is_string)
					}),
					"env" | "headers" | "http_headers" | "env_http_headers" => {
						value.as_object().is_some_and(|values| {
							values.values().all(Value::is_string)
						})
					}
					"enabled" | "required" => value.is_boolean(),
					"startup_timeout_ms" => {
						value.as_u64().is_some_and(|n| n > 0)
					}
					"startup_timeout_sec" | "tool_timeout_sec" => {
						value.as_f64().is_some_and(|n| n > 0.0)
					}
					_ => false,
				}
		})
}
fn hook(value: &Value) -> bool {
	value.as_object().is_some_and(|fields| {
		fields.get("type").and_then(Value::as_str) == Some("command")
			&& fields
				.get("command")
				.and_then(Value::as_str)
				.is_some_and(|s| !s.is_empty())
			&& fields.iter().all(|(key, value)| match key.as_str() {
				"type" | "command" | "statusMessage" => value.is_string(),
				"timeout" => value.as_u64().is_some_and(|n| n > 0),
				"async" | "once" | "asyncRewake" => value.is_boolean(),
				_ => false,
			})
	})
}
