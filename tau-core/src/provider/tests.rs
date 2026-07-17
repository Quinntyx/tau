use futures::StreamExt;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};

use super::ops::completion_request;
use super::{Provider, TauDelta};

#[tokio::test]
async fn mock_stream_yields_text_then_usage() {
    let model = MockCompletionModel::from_stream_turns([[
        MockStreamEvent::text("hello "),
        MockStreamEvent::text("world"),
        MockStreamEvent::final_response_with_total_tokens(42),
    ]]);
    let provider = Provider::Mock(model);

    let stream = provider
        .stream(completion_request("hi"))
        .await
        .expect("mock stream should start");

    let mut text = String::new();
    let mut usage_tokens = None;
    futures::pin_mut!(stream);
    while let Some(delta) = stream.next().await {
        match delta.unwrap() {
            TauDelta::Text(chunk) => text.push_str(&chunk),
            TauDelta::Usage(u) => usage_tokens = Some(u.total_tokens),
        }
    }
    assert_eq!(text, "hello world");
    assert_eq!(usage_tokens, Some(42));
}

#[test]
fn unknown_provider_errors() {
    let err = match Provider::new("nope", "m", "key", None) {
        Ok(_) => panic!("unknown provider should fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("unknown provider"));
}
