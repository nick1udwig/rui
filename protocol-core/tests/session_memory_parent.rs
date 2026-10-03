use rui_protocol_core::{resources::MemoryBudget, session::*};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

fn authorization(policy: u64) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: "owner".into(),
        policy_version: policy,
        permissions: Permissions {
            can_view: true,
            can_control: false,
            actions: Default::default(),
        },
    }
}
fn hello(id: u8) -> ClientHello {
    let config = SessionConfig::default();
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: config.versions,
        supported_profiles: config.supported_profiles,
        required_profiles: config.required_profiles,
        receiver_limits: config.receiver_limits,
        resume: None,
    }
}
fn make_broker(parent: MemoryBudget, limit: usize) -> SessionBroker {
    let mut config = SessionConfig::default();
    config.receiver_limits.view_memory_bytes = limit;
    SessionBroker::with_memory_budget([1; 16], [2; 16], config, parent).unwrap()
}
#[test]
fn parent_denial_leaves_view_charge_and_snapshot_unchanged() {
    let parent = MemoryBudget::new(1000);
    let mut broker = make_broker(parent.clone(), 2000);
    let session = broker
        .negotiate(Some(authorization(1)), hello(3), 0)
        .unwrap();
    broker
        .install_snapshot(&session.client, session.view_id, 1, 400)
        .unwrap();
    let used = broker
        .retained_memory_bytes(&session.client, session.view_id)
        .unwrap();
    let pressure = parent.reserve(600).unwrap();
    assert!(matches!(
        broker.reserve_view_memory(&session.client, session.view_id, 1),
        Err(SessionError::Oversized)
    ));
    assert!(matches!(
        broker.install_snapshot(&session.client, session.view_id, 2, 1),
        Err(SessionError::Oversized)
    ));
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        used
    );
    assert_eq!(parent.used(), 1000);
    drop(pressure);
    broker
        .acknowledge_applied(&session.client, session.view_id, 1, true)
        .unwrap();
    assert_eq!(parent.used(), 400);
    drop(broker);
    assert_eq!(parent.used(), 0);
}
#[test]
fn view_denial_releases_temporary_parent_reservation() {
    let parent = MemoryBudget::new(2000);
    let mut broker = make_broker(parent.clone(), 1000);
    let session = broker
        .negotiate(Some(authorization(1)), hello(3), 0)
        .unwrap();
    let held = broker
        .reserve_view_memory(&session.client, session.view_id, 1000)
        .unwrap();
    assert!(matches!(
        broker.reserve_view_memory(&session.client, session.view_id, 1),
        Err(SessionError::Oversized)
    ));
    assert_eq!(parent.used(), 1000);
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        1000
    );
    drop(held);
    assert_eq!(parent.used(), 0);
}
#[test]
fn escaped_io_payload_and_attachment_hold_parent_after_broker_disposal() {
    let parent = MemoryBudget::new(10000);
    let mut broker = make_broker(parent.clone(), 10000);
    let session = broker
        .negotiate(Some(authorization(1)), hello(3), 0)
        .unwrap();
    let io = broker
        .reserve_view_memory(&session.client, session.view_id, 1000)
        .unwrap();
    let attachment = broker
        .reserve_view_memory(&session.client, session.view_id, 2000)
        .unwrap()
        .attach([3; 16]);
    let attached = attachment.clone();
    let payload = broker
        .snapshot_publication_payload(
            &session.client,
            session.view_id,
            vec![7; 300],
            Some(attachment),
        )
        .unwrap();
    assert_eq!(
        parent.used(),
        1000 + 2000 + 300 + PUBLICATION_OVERHEAD_BYTES
    );
    broker.disconnect(&session.client, 1).unwrap();
    drop(broker);
    assert_eq!(
        parent.used(),
        1000 + 2000 + 300 + PUBLICATION_OVERHEAD_BYTES
    );
    drop(io);
    assert_eq!(parent.used(), 2000 + 300 + PUBLICATION_OVERHEAD_BYTES);
    drop(payload);
    assert_eq!(parent.used(), 2000);
    assert_eq!(attached.get::<[i32; 16]>(), Some(&[3; 16]));
    drop(attached);
    assert_eq!(parent.used(), 0);
}
#[test]
fn fresh_views_and_replacement_brokers_cannot_exceed_retained_parent_limit() {
    let parent = MemoryBudget::new(3000);
    let mut held = Vec::new();
    let mut broker = make_broker(parent.clone(), 10000);
    let mut last = None;
    for policy in 1..=3 {
        let session = broker
            .negotiate(Some(authorization(policy)), hello(3), 0)
            .unwrap();
        assert_ne!(last, Some(session.view_id));
        last = Some(session.view_id);
        held.push(
            broker
                .reserve_view_memory(&session.client, session.view_id, 1000)
                .unwrap(),
        );
    }
    assert_eq!(parent.used(), 3000);
    let fresh = broker
        .negotiate(Some(authorization(4)), hello(3), 0)
        .unwrap();
    assert!(matches!(
        broker.reserve_view_memory(&fresh.client, fresh.view_id, 1),
        Err(SessionError::Oversized)
    ));
    drop(broker);
    let mut replacement = make_broker(parent.clone(), 10000);
    let fresh = replacement
        .negotiate(Some(authorization(1)), hello(3), 0)
        .unwrap();
    assert!(matches!(
        replacement.reserve_view_memory(&fresh.client, fresh.view_id, 1),
        Err(SessionError::Oversized)
    ));
    held.pop();
    assert_eq!(parent.used(), 2000);
    let new = replacement
        .reserve_view_memory(&fresh.client, fresh.view_id, 1000)
        .unwrap();
    assert_eq!(parent.used(), parent.limit());
    drop(replacement);
    assert_eq!(parent.used(), 3000);
    drop(new);
    drop(held);
    assert_eq!(parent.used(), 0);
}
#[test]
fn attachment_value_destructor_runs_before_releasing_its_view_and_parent_charge() {
    struct Attached {
        budget: MemoryBudget,
        observed: Arc<AtomicUsize>,
        denied: Arc<AtomicBool>,
    }
    impl Drop for Attached {
        fn drop(&mut self) {
            self.observed.store(self.budget.used(), Ordering::Release);
            self.denied.store(
                self.budget.reserve(self.budget.limit()).is_err(),
                Ordering::Release,
            );
        }
    }
    let parent = MemoryBudget::new(1000);
    let observed = Arc::new(AtomicUsize::new(0));
    let denied = Arc::new(AtomicBool::new(false));
    let mut broker = make_broker(parent.clone(), 1000);
    let session = broker
        .negotiate(Some(authorization(1)), hello(3), 0)
        .unwrap();
    let attachment = broker
        .reserve_view_memory(&session.client, session.view_id, 1000)
        .unwrap()
        .attach(Attached {
            budget: parent.clone(),
            observed: observed.clone(),
            denied: denied.clone(),
        });
    drop(attachment);
    assert_eq!(observed.load(Ordering::Acquire), 1000);
    assert!(denied.load(Ordering::Acquire));
    assert_eq!(parent.used(), 0);
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        0
    );
}
