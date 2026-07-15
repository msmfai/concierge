//! Offline proofs for the AI harness: the tool schemas and the API request are
//! well-formed without a key or network. The live loop + the synthesize/verify
//! path are proven end-to-end against the real profile from the CLI.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use concierge_ai::{client, tools};

#[test]
fn tool_specs_are_well_formed() {
    let specs = tools::tool_specs();
    let arr = specs.as_array().expect("tools is an array");
    assert_eq!(arr.len(), 5);
    for t in arr {
        assert!(t.get("name").and_then(|n| n.as_str()).is_some());
        assert!(t.get("description").is_some());
        assert_eq!(
            t.get("input_schema")
                .and_then(|s| s.get("type"))
                .and_then(|x| x.as_str()),
            Some("object")
        );
    }
    // the escalation tool exists and takes a winner
    let resolve = arr.iter().find(|t| t["name"] == "resolve_asset").unwrap();
    let req = resolve["input_schema"]["required"].as_array().unwrap();
    assert!(req.iter().any(|r| r == "winner_mod"));
}

#[test]
fn request_body_is_well_formed_offline() {
    let msgs = serde_json::json!([{"role": "user", "content": "Goal: test"}]);
    let body = client::build_request(
        "claude-sonnet-5",
        &concierge_ai::system_prompt(),
        &msgs,
        &tools::tool_specs(),
    );
    assert_eq!(body["model"], "claude-sonnet-5");
    assert!(body["system"]
        .as_str()
        .unwrap()
        .contains("Rule of Least Power"));
    assert!(body["tools"].as_array().unwrap().len() == 5);
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body["max_tokens"].as_u64().unwrap() > 0);
}

#[test]
fn no_key_is_a_clean_error_not_a_panic() {
    // With no key configured the autonomous loop is gated, not crashing.
    // (In CI/this env there is no key; if a dev has one, skip the assertion.)
    match client::api_key() {
        Err(concierge_ai::Error::NoKey) => {}
        Ok(_) => eprintln!("a key is configured; autonomous loop is live"),
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[test]
fn ai_pipeline_refuses_the_impure_run_verb_offline() {
    // The AI proposes steps; the core must reject `run` before executing
    // anything — no network needed to prove the safety gate.
    let steps = serde_json::json!([{"run": ["echo", "hi"]}]);
    let tmp = std::env::temp_dir();
    let err = concierge_ai::tools::validate_pipeline(&steps, "x", &tmp)
        .expect_err("run verb must be rejected");
    assert!(err.to_string().contains("run"), "got: {err}");
}

#[test]
fn validate_pipeline_is_an_advertised_tool() {
    let names: Vec<String> = concierge_ai::tools::tool_specs()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    assert!(names.contains(&"validate_pipeline".to_owned()), "{names:?}");
}
