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
        capture.verify_native_glyph_evidence(receiver)?;
        let records = receiver.state()?.records.len();
        println!(
            "{} revision={} records={} retired={} memory={}",
            path.file_name().unwrap().to_string_lossy(),
            revision,
            records,
            receiver.state()?.retired.len(),
            receiver.memory_used()
        );
        captures += 1;
    }
    for receiver in receivers.values_mut() {
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
    println!("validated {captures} applied GTK snapshots across {} views; presentation awaits a renderer; all memory released", receivers.len());
    Ok(())
}
