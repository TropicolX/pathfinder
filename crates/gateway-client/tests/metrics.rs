//! This test was separated because the `metrics` crate uses a singleton recorder, so keeping a test
//! that relies on metric values in a separate binary makes more sense than using an inter-test
//! locking mechanism which can cause weird test failures without any obvious clue to what might
//! have caused those failures in the first place.

use futures::stream::StreamExt;
use pathfinder_common::{BlockId, BlockNumber};
use pretty_assertions_sorted::assert_eq;
use starknet_gateway_client::test_utils::{response_from, setup_with_varied_responses};
use starknet_gateway_client::{Client, GatewayApi};
use starknet_gateway_test_fixtures::{v0_11_0, v0_9_0};
use starknet_gateway_types::error::KnownStarknetErrorCode;
use std::future::Future;

#[tokio::test]
async fn all_counter_types_including_tags() {
    with_method(
        "get_block",
        |client, x| async move {
            let _ = client.block_deprecated(x).await;
        },
        (v0_9_0::block::GENESIS.to_owned(), 200),
    )
    .await;
    with_method(
        "get_state_update",
        |client, x| async move {
            let _ = client.state_update_deprecated(x).await;
        },
        (v0_11_0::state_update::GENESIS.to_owned(), 200),
    )
    .await;
}

async fn with_method<F, Fut, T>(method_name: &'static str, f: F, response: (String, u16))
where
    F: Fn(Client, BlockId) -> Fut,
    Fut: Future<Output = T>,
{
    use pathfinder_common::test_utils::metrics::{FakeRecorder, ScopedRecorderGuard};

    let recorder = FakeRecorder::new_for(&["get_block", "get_state_update"]);
    let handle = recorder.handle();

    // Automatically deregister the recorder
    let _guard = ScopedRecorderGuard::new(recorder);

    let responses = [
        // Any valid fixture
        response,
        // 1 Starknet error
        response_from(KnownStarknetErrorCode::BlockNotFound),
        // 2 decode errors
        (r#"{"not":"valid"}"#.to_owned(), 200),
        (r#"{"not":"valid, again"}"#.to_owned(), 200),
        // 3 of rate limiting
        ("you're being rate limited".to_owned(), 429),
        ("".to_owned(), 429),
        ("".to_owned(), 429),
    ];

    let (_jh, client) = setup_with_varied_responses([
        (
            format!("/feeder_gateway/{method_name}?blockNumber=123"),
            responses.clone(),
        ),
        (
            format!("/feeder_gateway/{method_name}?blockNumber=latest"),
            responses.clone(),
        ),
        (
            format!("/feeder_gateway/{method_name}?blockNumber=pending"),
            responses,
        ),
    ]);

    [BlockId::Number(BlockNumber::new_or_panic(123)); 7]
        .into_iter()
        .chain([BlockId::Latest; 7].into_iter())
        .chain([BlockId::Pending; 7].into_iter())
        .map(|x| f(client.clone(), x))
        .collect::<futures::stream::FuturesUnordered<_>>()
        .collect::<Vec<_>>()
        .await;

    // IMPORTANT
    //
    // We're not using any crate::sequencer::metrics consts here, because this is public API
    // and we'd like to catch if/when it changed (apparently due to a bug)
    [
        ("gateway_requests_total", None, None, 21),
        ("gateway_requests_total", Some("latest"), None, 7),
        ("gateway_requests_total", Some("pending"), None, 7),
        ("gateway_requests_failed_total", None, None, 18),
        ("gateway_requests_failed_total", Some("latest"), None, 6),
        ("gateway_requests_failed_total", Some("pending"), None, 6),
        ("gateway_requests_failed_total", None, Some("starknet"), 3),
        (
            "gateway_requests_failed_total",
            Some("latest"),
            Some("starknet"),
            1,
        ),
        (
            "gateway_requests_failed_total",
            Some("pending"),
            Some("starknet"),
            1,
        ),
        ("gateway_requests_failed_total", None, Some("decode"), 6),
        (
            "gateway_requests_failed_total",
            Some("latest"),
            Some("decode"),
            2,
        ),
        (
            "gateway_requests_failed_total",
            Some("pending"),
            Some("decode"),
            2,
        ),
        (
            "gateway_requests_failed_total",
            None,
            Some("rate_limiting"),
            9,
        ),
        (
            "gateway_requests_failed_total",
            Some("latest"),
            Some("rate_limiting"),
            3,
        ),
        (
            "gateway_requests_failed_total",
            Some("pending"),
            Some("rate_limiting"),
            3,
        ),
    ]
    .into_iter()
    .for_each(
        |(counter_name, tag, failure_reason, expected_count)| match (tag, failure_reason) {
            (None, None) => assert_eq!(
                handle.get_counter_value(counter_name, method_name),
                expected_count,
                "counter: {counter_name}, method: {method_name}"
            ),
            (None, Some(reason)) => assert_eq!(
                handle.get_counter_value_by_label(
                    counter_name,
                    [("method", method_name), ("reason", reason)]
                ),
                expected_count,
                "counter: {counter_name}, method: {method_name}, reason: {reason}"
            ),
            (Some(tag), None) => assert_eq!(
                handle.get_counter_value_by_label(
                    counter_name,
                    [("method", method_name), ("tag", tag)]
                ),
                expected_count,
                "counter: {counter_name}, method: {method_name}, tag: {tag}"
            ),
            (Some(tag), Some(reason)) => assert_eq!(
                handle.get_counter_value_by_label(
                    counter_name,
                    [("method", method_name), ("tag", tag), ("reason", reason)]
                ),
                expected_count,
                "counter: {counter_name}, method: {method_name}, tag: {tag}, reason: {reason}"
            ),
        },
    );
}
