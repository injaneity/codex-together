#![cfg(not(target_os = "windows"))]

use codex_core::HandoffSelectionRequest;
use codex_core::HandoffSelectionResult;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use responses::ev_assistant_message;
use responses::ev_completed;
use responses::sse;
use responses::start_mock_server;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_selection_filters_invalid_ids_and_preserves_priority() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message(
                "m2",
                r#"{"selected_ref_ids":["ctx:thread-search:thread-1:search-1","ctx:invalid","ctx:file:chatwidget","ctx:file:chatwidget"],"loading_prompt":"Inspect /context from the thread anchor, focus on the mounted files first, and continue the task."}"#,
            ),
            ev_completed("r1"),
        ]),
    )
    .await;

    let codex = test_codex().build(&server).await?.codex;
    let prompt = "\
Prepare a Codex handoff from the anchored context tree below.
Goal: Focus on the specific context files and continue inspecting how they can be improved.
Candidates:
- ◯ [file] Read codex-rs/context-graph/src/lib.rs :: ctx:file:context-graph
- ◯ [file] Read codex-rs/tui/src/chatwidget.rs :: ctx:file:chatwidget
- ◯ [insight] Search results in lib.rs :: ctx:thread-search:thread-1:search-1";

    let selection = codex
        .select_handoff_context(HandoffSelectionRequest {
            prompt: prompt.to_string(),
            allowed_ref_ids: vec![
                "ctx:file:context-graph".to_string(),
                "ctx:file:chatwidget".to_string(),
                "ctx:thread-search:thread-1:search-1".to_string(),
            ],
            max_selected_ref_ids: 2,
        })
        .await?;

    assert_eq!(
        selection,
        HandoffSelectionResult {
            selected_ref_ids: vec![
                "ctx:thread-search:thread-1:search-1".to_string(),
                "ctx:file:chatwidget".to_string(),
            ],
            loading_prompt:
                "Inspect /context from the thread anchor, focus on the mounted files first, and continue the task."
                    .to_string(),
        }
    );

    let body = mock.single_request().body_json();
    assert_eq!(
        body.pointer("/text/format/name"),
        Some(&Value::String("codex_output_schema".to_string()))
    );
    let body_text = serde_json::to_string(&body)?;
    assert!(body_text.contains("Prepare a Codex handoff from the anchored context tree below."));
    assert!(body_text.contains("ctx:file:chatwidget"));

    Ok(())
}
