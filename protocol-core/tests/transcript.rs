use rui_protocol_core::resources::{MemoryBudget, ResourceContext, ResourceScope};
use rui_protocol_core::transcript::*;
fn context() -> ResourceContext {
    ResourceContext {
        scope: ResourceScope {
            session_id: [1; 16],
            epoch: [2; 16],
            principal_id: [3; 16],
        },
        view_id: [4; 16],
    }
}
#[test]
fn ordinary_recording_retains_no_private_bytes_and_debug_never_echoes_payload() {
    let budget = MemoryBudget::new(1 << 20);
    let mut recorder = TranscriptRecorder::new(
        context(),
        RecordingPolicy::MetadataOnly,
        TranscriptLimits::default(),
        budget.clone(),
    )
    .unwrap();
    recorder
        .record(context(), Direction::ClientToHost, 3, b"PRIVATE-SENTINEL")
        .unwrap();
    let entry = recorder.entries(context()).unwrap().next().unwrap();
    assert!(entry.exact_bytes().is_none());
    assert!(!format!("{entry:?}").contains("PRIVATE-SENTINEL"));
    assert_eq!(budget.used(), 2048);
    recorder.revoke();
    assert_eq!(budget.used(), 0);
}
#[test]
fn authorized_exact_transcript_preserves_unknown_optional_bytes_and_identity_scope() {
    let budget = MemoryBudget::new(1 << 20);
    let mut recorder = TranscriptRecorder::new(
        context(),
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits::default(),
        budget.clone(),
    )
    .unwrap();
    let bytes = b"exact-protobuf-with-unknown-optional-fields";
    recorder
        .record(context(), Direction::HostToClient, 1, bytes)
        .unwrap();
    let entry = recorder.entries(context()).unwrap().next().unwrap();
    assert_eq!(entry.exact_bytes(), Some(bytes.as_slice()));
    assert!(!format!("{entry:?}").contains("exact-protobuf"));
    let mut other = context();
    other.scope.principal_id = [9; 16];
    assert!(matches!(
        recorder.entries(other),
        Err(TranscriptError::Unauthorized)
    ));
    assert_eq!(
        recorder.record(other, Direction::HostToClient, 1, b"x"),
        Err(TranscriptError::Unauthorized)
    );
    assert_eq!(budget.used(), 2048 + bytes.len());
    recorder.revoke();
    assert_eq!(budget.used(), 0);
    assert_eq!(
        recorder.record(context(), Direction::HostToClient, 1, b"x"),
        Err(TranscriptError::Unauthorized)
    );
}
#[test]
fn message_count_shared_memory_and_retention_fail_without_partial_record() {
    let budget = MemoryBudget::new(2050);
    let mut recorder = TranscriptRecorder::new(
        context(),
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits {
            records: 2,
            message_bytes: 2,
            retained_bytes: 4096,
        },
        budget.clone(),
    )
    .unwrap();
    assert_eq!(
        recorder.record(context(), Direction::HostToClient, 1, b"big"),
        Err(TranscriptError::Limit)
    );
    assert_eq!(
        recorder.record(context(), Direction::HostToClient, 1, b"ok"),
        Ok(1)
    );
    assert!(matches!(
        recorder.record(context(), Direction::HostToClient, 1, b"x"),
        Err(TranscriptError::Limit) | Err(TranscriptError::Memory(_))
    ));
    assert_eq!(recorder.entries(context()).unwrap().count(), 1);
    drop(recorder);
    assert_eq!(budget.used(), 0);
    let mut recorder = TranscriptRecorder::new(
        context(),
        RecordingPolicy::MetadataOnly,
        TranscriptLimits {
            records: 1,
            ..Default::default()
        },
        budget,
    )
    .unwrap();
    recorder
        .record(context(), Direction::HostToClient, 1, b"a")
        .unwrap();
    assert_eq!(
        recorder.record(context(), Direction::HostToClient, 1, b"b"),
        Err(TranscriptError::Limit)
    );
}
