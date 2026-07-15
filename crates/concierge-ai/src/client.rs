//! Thin Anthropic Messages API client for the agent loop. Key is client-side
//! (`~/.config/concierge/anthropic-api-key` or `ANTHROPIC_API_KEY`), mirroring the
//! Nexus key — never embedded, never mirrored. Swappable: the loop calls
//! `send`, so a different backend is a drop-in.

use crate::Error;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";

/// Read the API key (config file first, then env). `Err(NoKey)` if absent — the
/// autonomous loop is gated on the user supplying one; everything else runs
/// without it.
pub fn api_key() -> Result<String, Error> {
    let path = concierge_platform::config_file("anthropic-api-key");
    if let Ok(k) = std::fs::read_to_string(&path) {
        let k = k.trim().to_owned();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or(Error::NoKey)
}

/// Send one Messages request (with tools) and return the parsed response JSON.
/// The caller drives the `tool_use` -> `tool_result` loop.
pub fn send(
    key: &str,
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 2048,
        "system": system,
        "tools": tools,
        "messages": messages,
    });
    let resp = ureq::post(ENDPOINT)
        .set("x-api-key", key)
        .set("anthropic-version", VERSION)
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(|e| Error::Api(e.to_string()))?;
    resp.into_json::<serde_json::Value>()
        .map_err(|e| Error::Api(e.to_string()))
}

/// Build the well-formed request body without sending — so the request shape is
/// unit-testable offline (no key, no network).
#[must_use]
pub fn build_request(
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 2048,
        "system": system,
        "tools": tools,
        "messages": messages,
    })
}
