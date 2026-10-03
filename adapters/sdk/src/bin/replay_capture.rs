use rui_producer_sdk::CaptureArtifact;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: replay_capture CAPTURE_DIRECTORY")?,
    );
    let mut artifacts = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|value| value == "json")
                && path
                    .file_name()
                    .is_some_and(|name| name != "corpus-summary.json")
        })
        .collect::<Vec<_>>();
    artifacts.sort();
    let mut receivers = BTreeMap::new();
    let mut captures = 0;
    for path in artifacts {
        let capture = CaptureArtifact::read(&path)?;
        let context = capture.context();
        if let std::collections::btree_map::Entry::Vacant(entry) = receivers.entry(context.view_id)
        {
            entry.insert(capture.receiver()?);
        }
        let receiver = receivers.get_mut(&context.view_id).unwrap();
        let revision = capture.apply_trusted_fixture(receiver)?;
        assert!(receiver.resources_ready()?);
        let audit = if capture.manifest()["exporter_counters"]["schema"] == 2
            || capture.manifest()["scene"].is_array()
        {
            Some(capture.verify_vector_audit(receiver)?)
        } else {
            // Early retained foundation manifests lack full source scene data.
            // Preserve their glyph check and label this limited historical path.
            capture.verify_native_glyph_evidence(receiver)?;
            None
        };
        let records = receiver.state()?.records.len();
        println!(
            "{} revision={} records={} retired={} memory={} vector-audit={}",
            path.file_name().unwrap().to_string_lossy(),
            revision,
            records,
            receiver.state()?.retired.len(),
            receiver.memory_used(),
            audit.map_or("historical-glyph-only".to_owned(), |a| format!(
                "bodies:{} gaps:{} observed-counters:{}",
                a.resource_bodies, a.unsupported_subtrees, a.observed_counter_schema
            ))
        );
        captures += 1;
    }
    for receiver in receivers.values_mut() {
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
    println!(
        "validated {captures} applied GTK snapshots across {} views; presentation awaits a renderer; all memory released",
        receivers.len()
    );
    Ok(())
}
