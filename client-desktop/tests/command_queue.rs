use rui_client_desktop::command_queue::*;
use rui_protocol_core::{commands::CommandFamily, resources::MemoryBudget};
#[test]
fn exact_pending_commands_are_charged_before_encoding_and_complete_independently() {
    let budget = MemoryBudget::new(1024 * 1024);
    let mut queue = CommandQueue::new([1; 16], budget.clone(), 1024).unwrap();
    let metadata = budget.used();
    let secret = b"PRIVATE-QUEUED-COMMAND";
    for family in [
        CommandFamily::Semantic,
        CommandFamily::RawInput,
        CommandFamily::TextComposition,
        CommandFamily::Resize,
    ] {
        let before = budget.used();
        let sequence = queue.next_sequence();
        assert_eq!(
            queue
                .encode(family, secret.len(), |bytes| {
                    assert!(budget.used() > before);
                    bytes.extend_from_slice(secret);
                    Ok(())
                })
                .unwrap(),
            sequence
        );
    }
    assert_eq!(queue.next_sequence(), 5);
    assert_eq!(queue.len(), 4);
    let diagnostic = format!("{:?}", queue.latest().unwrap());
    assert!(!diagnostic.contains("PRIVATE"));
    assert!(!diagnostic.contains(&format!("{secret:?}")));
    assert!(queue.complete(2));
    assert!(!queue.complete(2));
    assert_eq!(
        queue.commands().map(|c| c.sequence()).collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
    for command in queue.commands() {
        assert_eq!(command.bytes(), secret);
    }
    for sequence in [1, 3, 4] {
        assert!(queue.complete(sequence));
    }
    assert_eq!(budget.used(), metadata);
    drop(queue);
    assert_eq!(budget.used(), 0);
}
#[test]
fn exhaustion_never_invokes_encoder_or_consumes_sequence_and_rebind_is_atomic() {
    let budget = MemoryBudget::new(1024 * 1024);
    let mut queue = CommandQueue::new([1; 16], budget.clone(), 1024).unwrap();
    queue
        .encode(CommandFamily::Semantic, 4, |b| {
            b.extend_from_slice(b"1234");
            Ok(())
        })
        .unwrap();
    let held = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert_eq!(
        queue.encode(CommandFamily::RawInput, 4, |_| panic!(
            "encoder must not run"
        )),
        Err(QueueError::Limit)
    );
    assert_eq!(queue.next_sequence(), 2);
    drop(held);
    assert_eq!(
        queue.rebind([1; 16], MemoryBudget::new(1024 * 1024), 3),
        Err(QueueError::IncompatibleLimits)
    );
    let tiny = MemoryBudget::new(1);
    assert_eq!(
        queue.rebind([1; 16], tiny.clone(), 1024),
        Err(QueueError::Limit)
    );
    assert_eq!(tiny.used(), 0);
    assert_eq!(queue.latest().unwrap().bytes(), b"1234");
    let replacement = MemoryBudget::new(1024 * 1024);
    assert!(!queue.rebind([1; 16], replacement.clone(), 1024).unwrap());
    assert_eq!(budget.used(), 0);
    assert_eq!(queue.next_sequence(), 2);
    assert!(queue.rebind([2; 16], replacement.clone(), 1024).unwrap());
    assert!(queue.is_empty());
    assert_eq!(queue.next_sequence(), 1);
    for _ in 0..MAX_PENDING_COMMANDS {
        queue.encode(CommandFamily::Resize, 0, |_| Ok(())).unwrap();
    }
    assert_eq!(
        queue.encode(CommandFamily::Resize, 0, |_| panic!(
            "no unbounded zero-byte entries"
        )),
        Err(QueueError::Limit)
    );
    drop(queue);
    assert_eq!(replacement.used(), 0);
}
