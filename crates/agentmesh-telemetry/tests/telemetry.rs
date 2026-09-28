//! Trace, metric cardinality, aggregation, and structured event tests.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_error::ErrorCode;
use agentmesh_telemetry::{Attributes, Level, TelemetryEvent, TelemetryRecorder, TraceContext};

fn attributes(region: &str) -> Attributes {
    Attributes::new(
        BTreeMap::from([("region".into(), region.into())]),
        &BTreeSet::from(["region".into()]),
    )
    .unwrap()
}

#[test]
fn traceparent_round_trips_and_rejects_zero_ids() {
    let root = TraceContext::root();
    assert_eq!(
        TraceContext::parse(&root.traceparent()).unwrap().trace_id(),
        root.trace_id()
    );
    assert_eq!(
        TraceContext::parse("00-00000000000000000000000000000000-0000000000000000-01")
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn metrics_aggregate_and_cardinality_is_bounded() {
    let recorder = TelemetryRecorder::new(1, 2).unwrap();
    recorder
        .record("request.duration_ms", attributes("east"), 10)
        .unwrap();
    recorder
        .record("request.duration_ms", attributes("east"), 20)
        .unwrap();
    assert_eq!(recorder.metrics()[0].sum, 30);
    assert_eq!(
        recorder
            .record("request.duration_ms", attributes("west"), 1)
            .unwrap_err()
            .code(),
        ErrorCode::StorageUnavailable
    );
    assert_eq!(recorder.dropped().0, 1);
}

#[test]
fn sensitive_attributes_are_rejected_and_events_are_bounded() {
    let error = Attributes::new(
        BTreeMap::from([("api_token".into(), "secret".into())]),
        &BTreeSet::from(["api_token".into()]),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigurationInvalid);
    let recorder = TelemetryRecorder::new(1, 1).unwrap();
    for code in ["first", "second"] {
        recorder
            .emit(TelemetryEvent {
                level: Level::Info,
                name: "request.completed".into(),
                message_code: code.into(),
                attributes: attributes("east"),
                trace_id: None,
                at_millis: 1,
            })
            .unwrap();
    }
    assert_eq!(recorder.drain_events()[0].message_code, "second");
    assert_eq!(recorder.dropped().1, 1);
}
