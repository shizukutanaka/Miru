//! MCP tool definitions.
//!
//! Each tool maps to a Miru AgentSession capability + a transport action.
//! Naming convention: `miru_<verb>_<noun>` to avoid collision with other
//! MCP servers Claude might have loaded simultaneously.

use crate::protocol::ToolDefinition;
use serde_json::json;

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "miru_screen_capture",
            description: "Capture the current screen of the connected Miru host. \
                          Returns a base64 PNG image. Use this to see what's on the screen \
                          before deciding what action to take.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "display": {
                        "type": "integer",
                        "description": "Display index (0 = primary). Optional, defaults to 0.",
                        "default": 0,
                    },
                    "region": {
                        "type": "object",
                        "description": "Optional sub-region to capture in normalized coords (0-1).",
                        "properties": {
                            "x": {"type": "number"},
                            "y": {"type": "number"},
                            "width": {"type": "number"},
                            "height": {"type": "number"},
                        },
                    },
                },
            }),
        },
        ToolDefinition {
            name: "miru_mouse_move",
            description: "Move the mouse cursor to the given normalized position (0-1). \
                          Coordinate (0,0) is top-left, (1,1) is bottom-right.",
            input_schema: json!({
                "type": "object",
                "required": ["x", "y"],
                "properties": {
                    "x": {"type": "number", "minimum": 0, "maximum": 1},
                    "y": {"type": "number", "minimum": 0, "maximum": 1},
                },
            }),
        },
        ToolDefinition {
            name: "miru_mouse_click",
            description: "Click a mouse button at the given normalized position. \
                          Button defaults to 'left'. Use 'double' to double-click.",
            input_schema: json!({
                "type": "object",
                "required": ["x", "y"],
                "properties": {
                    "x": {"type": "number"},
                    "y": {"type": "number"},
                    "button": {
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                        "default": "left",
                    },
                    "double": {"type": "boolean", "default": false},
                },
            }),
        },
        ToolDefinition {
            name: "miru_scroll",
            description: "Scroll the mouse wheel at the given position. Positive dy = scroll up.",
            input_schema: json!({
                "type": "object",
                "required": ["dy"],
                "properties": {
                    "x": {"type": "number"},
                    "y": {"type": "number"},
                    "dx": {"type": "number", "default": 0},
                    "dy": {"type": "number"},
                },
            }),
        },
        ToolDefinition {
            name: "miru_key_type",
            description: "Type the given text string. Use this for plain text entry — \
                          for keyboard shortcuts use miru_key_combo instead.",
            input_schema: json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text": {"type": "string", "maxLength": 4096},
                },
            }),
        },
        ToolDefinition {
            name: "miru_key_combo",
            description: "Send a keyboard shortcut. Format: modifiers separated by '+', e.g. \
                          'Ctrl+C', 'Cmd+Shift+T', 'Alt+Tab'. Modifiers: Ctrl, Cmd (Meta), \
                          Shift, Alt. Then a single key.",
            input_schema: json!({
                "type": "object",
                "required": ["combo"],
                "properties": {
                    "combo": {"type": "string"},
                },
            }),
        },
        ToolDefinition {
            name: "miru_clipboard_read",
            description: "Read the host's clipboard contents. Requires confirmation each call \
                          (clipboard may contain sensitive data).",
            input_schema: json!({
                "type": "object",
                "properties": {},
            }),
        },
        ToolDefinition {
            name: "miru_clipboard_write",
            description: "Set the host's clipboard contents to the given text.",
            input_schema: json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text": {"type": "string", "maxLength": 1048576},
                },
            }),
        },
        ToolDefinition {
            name: "miru_open_url",
            description: "Open a URL in the host's default web browser.",
            input_schema: json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "format": "uri",
                        "pattern": "^https?://",
                    },
                },
            }),
        },
        ToolDefinition {
            name: "miru_status",
            description: "Get the connection status, agent token expiry, and recent activity.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
    ]
}
