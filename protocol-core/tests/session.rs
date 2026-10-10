use rui_protocol_core::commands::*;
use rui_protocol_core::session::*;
use std::collections::BTreeSet;

fn actions() -> BTreeSet<String> {
    ["Activate", "ReplaceText", "Focus"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn auth(principal: &str, control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: principal.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: actions(),
        },
    }
}
fn hello(client: u8) -> ClientHello {
    ClientHello {
        client_instance_id: [client; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    }
}
fn broker() -> SessionBroker {
    SessionBroker::new([1; 16], [2; 16], SessionConfig::default()).unwrap()
}
#[test]
fn trusted_adapter_reservations_share_view_budget_and_survive_scope_disconnect() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    let hold = broker
        .reserve_view_memory(
            &session.client,
            session.view_id,
            session.limits.view_memory_bytes,
        )
        .unwrap();
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        session.limits.view_memory_bytes
    );
    assert!(matches!(
        broker.reserve_view_memory(&session.client, session.view_id, 1),
        Err(SessionError::Oversized)
    ));
    let wrong = AuthenticatedClient {
        principal: "mallory".into(),
        ..session.client.clone()
    };
    assert!(matches!(
        broker.reserve_view_memory(&wrong, session.view_id, 1),
        Err(SessionError::WrongPrincipal)
    ));
    broker.disconnect(&session.client, 1).unwrap();
    assert!(broker
        .reserve_view_memory(&session.client, session.view_id, 1)
        .is_err());
    drop(hold);
    let session = connect(&mut broker, 3, "alice", true);
    assert_eq!(
        broker
            .peak_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        session.limits.view_memory_bytes
    );
    assert_eq!(
        broker.peak_memory_bytes(&wrong, session.view_id),
        Err(SessionError::WrongPrincipal)
    );
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        0
    );
}
fn connect(
    broker: &mut SessionBroker,
    client: u8,
    principal: &str,
    control: bool,
) -> NegotiatedSession {
    broker
        .negotiate(Some(auth(principal, control)), hello(client), 0)
        .unwrap()
}
fn usable(broker: &mut SessionBroker, session: &NegotiatedSession, revision: u64) {
    broker
        .install_snapshot(&session.client, session.view_id, revision, 1)
        .unwrap();
    broker
        .acknowledge_applied(&session.client, session.view_id, revision, true)
        .unwrap();
    broker
        .acknowledge_presented(&session.client, session.view_id, revision)
        .unwrap();
}
#[test]
fn actual_presentation_marker_is_bound_and_cleared_without_fabrication() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    broker
        .install_snapshot(&session.client, session.view_id, 1, 1)
        .unwrap();
    broker
        .acknowledge_applied(&session.client, session.view_id, 1, true)
        .unwrap();
    broker
        .authorize_presented(&session.client, session.view_id, 1)
        .unwrap();
    assert_eq!(
        broker.presented_revision(&session.client, session.view_id),
        Ok(None)
    );
    broker
        .acknowledge_presented(&session.client, session.view_id, 1)
        .unwrap();
    assert_eq!(
        broker.presented_revision(&session.client, session.view_id),
        Ok(Some(1))
    );
    let wrong = AuthenticatedClient {
        principal: "other".into(),
        ..session.client.clone()
    };
    assert_eq!(
        broker.presented_revision(&wrong, session.view_id),
        Err(SessionError::WrongPrincipal)
    );
    assert_eq!(
        broker.presented_revision(&session.client, [99; 16]),
        Err(SessionError::WrongView)
    );
    broker
        .mark_resources_missing(&session.client, session.view_id)
        .unwrap();
    assert_eq!(
        broker.presented_revision(&session.client, session.view_id),
        Ok(None)
    );
    broker.disconnect(&session.client, 1).unwrap();
    assert!(broker
        .presented_revision(&session.client, session.view_id)
        .is_err());
}
#[test]
fn trusted_confirmed_releases_bypass_readiness_but_never_clear_another_owner() {
    let mut broker = broker();
    let a = connect(&mut broker, 3, "alice", true);
    let b = connect(&mut broker, 4, "bob", true);
    usable(&mut broker, &a, 1);
    usable(&mut broker, &b, 1);
    let old = broker
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let key = PhysicalKey {
        namespace: "linux.evdev".into(),
        code: 30,
    };
    broker.record_key(&old, key.clone(), true, 0).unwrap();
    broker.record_button(&old, 1, true, 0).unwrap();
    broker.record_touch(&old, 9, true, 0).unwrap();
    broker.mark_resources_missing(&a.client, a.view_id).unwrap();
    assert!(broker.record_key(&old, key.clone(), false, 1).is_err());
    let mut wrong = old.clone();
    wrong.client.principal = "other".into();
    assert!(broker.confirm_key_released(&wrong, &key, 1).is_err());
    wrong = old.clone();
    wrong.view_id = [99; 16];
    assert!(broker.confirm_button_released(&wrong, 1, 1).is_err());
    broker.confirm_key_released(&old, &key, 1).unwrap();
    broker.confirm_button_released(&old, 1, 1).unwrap();
    broker.confirm_touch_released(&old, 9, 1).unwrap();
    assert_eq!(broker.presented_revision(&a.client, a.view_id), Ok(None));
    assert_eq!(broker.current_lease(1).unwrap().lease_id, old.lease_id);
    broker.local_activity(2);
    let cleanup = broker.take_cleanup().unwrap();
    assert!(cleanup.keys.is_empty() && cleanup.buttons.is_empty() && cleanup.touches.is_empty());
    let new = broker
        .acquire_control(&b.client, b.view_id, [2; 16], 3)
        .unwrap();
    broker.record_key(&new, key.clone(), true, 3).unwrap();
    broker.record_button(&new, 1, true, 3).unwrap();
    broker.record_touch(&new, 9, true, 3).unwrap();
    assert!(broker.confirm_key_released(&old, &key, 3).is_err());
    assert!(broker.confirm_button_released(&old, 1, 3).is_err());
    assert!(broker.confirm_touch_released(&old, 9, 3).is_err());
    // Guessing a new generation does not make the old client its owner.
    wrong = old.clone();
    wrong.lease_id = new.lease_id;
    wrong.generation = new.generation;
    assert!(broker.confirm_key_released(&wrong, &key, 3).is_err());
    broker.local_activity(4);
    let cleanup = broker.take_cleanup().unwrap();
    assert_eq!(cleanup.keys, vec![key]);
    assert_eq!(cleanup.buttons, vec![1]);
    assert_eq!(cleanup.touches, vec![9]);
}
fn command(session: &NegotiatedSession, lease: &ControlLease) -> SemanticCommand {
    SemanticCommand {
        epoch: session.epoch,
        view_id: session.view_id,
        client_instance_id: session.client.client_instance_id,
        command_seq: 1,
        control_lease_id: lease.lease_id,
        control_generation: lease.generation,
        target_id: 10,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 30,
        preconditions: Preconditions::default(),
        arguments: Arguments::None,
    }
}

#[test]
fn neg_001_authentication_precedes_projection_and_views_start_read_only() {
    let mut broker = broker();
    assert!(matches!(
        broker.negotiate(None, hello(3), 0),
        Err(SessionError::Unauthenticated)
    ));
    let fake = AuthenticatedClient {
        principal: "alice".into(),
        client_instance_id: [3; 16],
    };
    assert_eq!(
        broker.authorize_projection(&fake, [1; 16]),
        Err(SessionError::Unauthenticated)
    );
    let session = connect(&mut broker, 3, "alice", true);
    assert_eq!(session.profiles, SessionConfig::default().required_profiles);
    assert!(broker.current_lease(0).is_none());
    assert!(broker
        .authorize_projection(&session.client, session.view_id)
        .is_ok());
}
#[test]
fn neg_002_unknown_required_profile_and_missing_strict_profile_disclose_no_view() {
    let mut broker = broker();
    let mut hello = hello(3);
    hello.required_profiles.insert("unknown.render.v99".into());
    assert!(matches!(
        broker.negotiate(Some(auth("alice", true)), hello, 0),
        Err(SessionError::UnsupportedProfile)
    ));
    let mut hello = self::hello(3);
    hello.supported_profiles.remove("content.no-raster.v1");
    assert!(matches!(
        broker.negotiate(Some(auth("alice", true)), hello, 0),
        Err(SessionError::UnsupportedProfile)
    ));
    let session = connect(&mut broker, 3, "alice", true);
    assert_eq!(&session.view_id[8..], &1_u64.to_be_bytes());
}
#[test]
fn neg_005_receiver_ceilings_are_negotiated_downward() {
    let mut broker = broker();
    let mut hello = hello(3);
    hello.receiver_limits.transaction_bytes = 3;
    hello.receiver_limits.snapshot_bytes = 4;
    hello.receiver_limits.view_memory_bytes = 5;
    hello.receiver_limits.envelope_bytes = usize::MAX;
    let session = broker
        .negotiate(Some(auth("alice", true)), hello, 0)
        .unwrap();
    assert_eq!(session.limits.transaction_bytes, 3);
    assert_eq!(session.limits.view_memory_bytes, 5);
    assert_eq!(
        session.limits.envelope_bytes,
        ReceiverLimits::default().envelope_bytes
    );
    assert_eq!(
        broker.install_snapshot(&session.client, session.view_id, 1, 5),
        Err(SessionError::Oversized)
    );
    usable(&mut broker, &session, 1);
    assert_eq!(
        broker.publish(&session.client, session.view_id, 1, 2, vec![0; 4], 0),
        Err(SessionError::Oversized)
    );
}
#[test]
fn input_requires_usable_presented_state_in_current_connection() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    broker
        .install_snapshot(&session.client, session.view_id, 30, 1)
        .unwrap();
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 0),
        Err(SessionError::Unauthorized)
    );
    broker
        .acknowledge_applied(&session.client, session.view_id, 30, true)
        .unwrap();
    assert_eq!(
        broker.acknowledge_presented(&session.client, session.view_id, 0),
        Err(SessionError::NotPresentable)
    );
    assert_eq!(
        broker.acknowledge_presented(&session.client, session.view_id, 1),
        Err(SessionError::NotPresentable)
    );
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 0),
        Err(SessionError::Unauthorized)
    );
    broker
        .acknowledge_presented(&session.client, session.view_id, 30)
        .unwrap();
    assert!(broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .is_ok());
}
#[test]
fn mul_001_only_one_authorized_client_can_control() {
    let mut broker = broker();
    let a = connect(&mut broker, 3, "alice", true);
    let b = connect(&mut broker, 4, "bob", false);
    usable(&mut broker, &a, 30);
    usable(&mut broker, &b, 30);
    let lease = broker
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    assert_eq!(
        broker.acquire_control(&b.client, b.view_id, [2; 16], 0),
        Err(SessionError::Unauthorized)
    );
    assert_eq!(
        broker.validate_control(&a.client, &command(&a, &lease), 1),
        Ok(())
    );
    let forged = command(&b, &lease);
    assert_eq!(
        broker.validate_control(&b.client, &forged, 1),
        Err(Rejection::Unauthorized)
    );
    assert!(broker.authorize_projection(&b.client, b.view_id).is_ok());
}
#[test]
fn mul_002_local_priority_unwinds_input_and_rejects_old_generation() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let old = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    broker
        .record_key(
            &old,
            PhysicalKey {
                namespace: "evdev".into(),
                code: 30,
            },
            true,
            1,
        )
        .unwrap();
    broker.record_button(&old, 1, true, 1).unwrap();
    broker.record_touch(&old, 9, true, 1).unwrap();
    broker.set_composition(&old, Some([8; 16]), 1).unwrap();
    broker.local_activity(2);
    assert_eq!(
        broker.validate_control(&session.client, &command(&session, &old), 2),
        Err(Rejection::StaleControl)
    );
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [2; 16], 2),
        Err(SessionError::ControlBusy)
    );
    let cleanup = broker.take_cleanup().unwrap();
    assert_eq!(cleanup.reason, CleanupReason::LocalActivity);
    assert_eq!(cleanup.keys.len(), 1);
    assert_eq!(cleanup.buttons, vec![1]);
    assert_eq!(cleanup.touches, vec![9]);
    assert_eq!(cleanup.composition, Some([8; 16]));
    let new = broker
        .acquire_control(&session.client, session.view_id, [2; 16], 2)
        .unwrap();
    assert!(new.generation > old.generation);
    assert_ne!(new.lease_id, old.lease_id);
    assert_eq!(
        broker.validate_control(&session.client, &command(&session, &old), 2),
        Err(Rejection::StaleControl)
    );
}
#[test]
fn disconnect_expiry_and_revocation_all_produce_cleanup() {
    for reason in [
        CleanupReason::Disconnect,
        CleanupReason::Expired,
        CleanupReason::ExplicitRevocation,
    ] {
        let mut broker = broker();
        let session = connect(&mut broker, 3, "alice", true);
        usable(&mut broker, &session, 30);
        let lease = broker
            .acquire_control(&session.client, session.view_id, [1; 16], 0)
            .unwrap();
        broker
            .record_key(
                &lease,
                PhysicalKey {
                    namespace: "evdev".into(),
                    code: 30,
                },
                true,
                1,
            )
            .unwrap();
        broker.record_button(&lease, 1, true, 1).unwrap();
        broker.set_composition(&lease, Some([8; 16]), 1).unwrap();
        match reason {
            CleanupReason::Disconnect => broker.disconnect(&session.client, 2).unwrap(),
            CleanupReason::Expired => broker.tick(5000),
            _ => broker.revoke_control(2),
        }
        let cleanup = broker.take_cleanup().unwrap();
        assert_eq!(cleanup.reason, reason);
        assert_eq!(cleanup.keys.len(), 1);
        assert_eq!(cleanup.buttons, vec![1]);
        assert_eq!(cleanup.composition, Some([8; 16]));
        assert!(broker.take_cleanup().is_none());
        assert!(broker.current_lease(1).is_none());
    }
}
#[test]
fn mul_005_acquisition_renewal_release_are_idempotent_without_resurrection() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 100)
        .unwrap();
    assert_eq!(
        broker
            .acquire_control(&session.client, session.view_id, [1; 16], 101)
            .unwrap(),
        lease
    );
    assert_eq!(
        broker
            .acquire_control(&session.client, session.view_id, [2; 16], 102)
            .unwrap(),
        lease
    );
    let renewed = broker
        .renew_control(
            &session.client,
            session.view_id,
            [3; 16],
            lease.lease_id,
            lease.generation,
            1000,
        )
        .unwrap();
    assert_eq!(renewed.expires_at_ms, 6000);
    assert_eq!(
        broker
            .renew_control(
                &session.client,
                session.view_id,
                [3; 16],
                lease.lease_id,
                lease.generation,
                2000
            )
            .unwrap(),
        renewed
    );
    assert_eq!(broker.current_lease(2000).unwrap().expires_at_ms, 6000);
    assert_eq!(
        broker.release_control(
            &session.client,
            [3; 16],
            lease.lease_id,
            lease.generation,
            2000
        ),
        Err(SessionError::ChangedRequest)
    );
    broker
        .release_control(
            &session.client,
            [4; 16],
            lease.lease_id,
            lease.generation,
            2000,
        )
        .unwrap();
    broker
        .release_control(
            &session.client,
            [4; 16],
            lease.lease_id,
            lease.generation,
            2001,
        )
        .unwrap();
    assert_eq!(
        broker.take_cleanup().unwrap().reason,
        CleanupReason::Released
    );
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 2002),
        Err(SessionError::StaleRequest)
    );
    let next = broker
        .acquire_control(&session.client, session.view_id, [5; 16], 2002)
        .unwrap();
    broker
        .release_control(
            &session.client,
            [6; 16],
            lease.lease_id,
            lease.generation,
            2003,
        )
        .unwrap();
    assert_eq!(broker.current_lease(2003).unwrap(), next);
    assert_eq!(
        broker.renew_control(
            &session.client,
            session.view_id,
            [7; 16],
            lease.lease_id,
            lease.generation,
            2003
        ),
        Err(SessionError::StaleControl)
    );
}
#[test]
fn expired_acquire_and_clock_rollback_never_revive_old_control() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 1000)
        .unwrap();
    broker.tick(6000);
    broker.take_cleanup().unwrap();
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 0),
        Err(SessionError::StaleRequest)
    );
    assert_eq!(
        broker.renew_control(
            &session.client,
            session.view_id,
            [2; 16],
            lease.lease_id,
            lease.generation,
            0
        ),
        Err(SessionError::StaleControl)
    );
    let next = broker
        .acquire_control(&session.client, session.view_id, [3; 16], 0)
        .unwrap();
    assert_eq!(next.expires_at_ms, 11_000);
    assert!(next.generation > lease.generation);
}
#[test]
fn rec_001_resume_replays_retained_chain_without_restoring_lease_or_input() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    broker.record_button(&lease, 1, true, 1).unwrap();
    for revision in 31..=35 {
        broker
            .publish(
                &session.client,
                session.view_id,
                revision - 1,
                revision,
                vec![revision as u8],
                1,
            )
            .unwrap();
        drop(
            broker
                .take_outbound(&session.client, session.view_id)
                .unwrap(),
        );
    }
    broker.disconnect(&session.client, 2).unwrap();
    let mut hello = hello(3);
    hello.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: session.view_id,
        acknowledged_revision: 30,
    });
    let resumed = broker
        .negotiate(Some(auth("alice", true)), hello, 3)
        .unwrap();
    match resumed.resume {
        ResumeDisposition::Accepted {
            from_revision,
            through_revision,
            transactions,
            requires_resources,
        } => {
            assert_eq!((from_revision, through_revision), (30, 35));
            assert!(!requires_resources);
            assert_eq!(
                transactions.iter().map(|p| p[0]).collect::<Vec<_>>(),
                vec![31, 32, 33, 34, 35]
            );
        }
        _ => panic!("retained history must resume"),
    }
    assert!(broker.current_lease(3).is_none());
    assert_eq!(broker.take_cleanup().unwrap().buttons, vec![1]);
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [2; 16], 3),
        Err(SessionError::Unauthorized)
    );
    broker
        .acknowledge_presented(&session.client, session.view_id, 30)
        .unwrap();
    assert!(broker
        .acquire_control(&session.client, session.view_id, [2; 16], 3)
        .is_ok());
}
#[test]
fn rec_002_evicted_history_requires_coherent_checkpoint() {
    let config = SessionConfig {
        max_history_records: 2,
        ..SessionConfig::default()
    };
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    for revision in 31..=35 {
        broker
            .publish(
                &session.client,
                session.view_id,
                revision - 1,
                revision,
                vec![revision as u8],
                0,
            )
            .unwrap();
        drop(
            broker
                .take_outbound(&session.client, session.view_id)
                .unwrap(),
        );
    }
    let mut hello = hello(3);
    hello.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: session.view_id,
        acknowledged_revision: 30,
    });
    let resumed = broker
        .negotiate(Some(auth("alice", true)), hello, 0)
        .unwrap();
    assert_eq!(
        resumed.resume,
        ResumeDisposition::ResetRequired(ResetReason::HistoryUnavailable)
    );
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 0),
        Err(SessionError::Unauthorized)
    );
    usable(&mut broker, &resumed, 35);
    assert!(broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .is_ok());
}
#[test]
fn rec_004_missing_resources_prevent_presented_ack() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    broker
        .mark_resources_missing(&session.client, session.view_id)
        .unwrap();
    let mut hello = hello(3);
    hello.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: session.view_id,
        acknowledged_revision: 30,
    });
    let resumed = broker
        .negotiate(Some(auth("alice", true)), hello, 0)
        .unwrap();
    assert!(matches!(
        resumed.resume,
        ResumeDisposition::Accepted {
            requires_resources: true,
            ..
        }
    ));
    assert_eq!(
        broker.acknowledge_presented(&session.client, session.view_id, 30),
        Err(SessionError::NotPresentable)
    );
    broker
        .acknowledge_applied(&session.client, session.view_id, 30, true)
        .unwrap();
    broker
        .acknowledge_presented(&session.client, session.view_id, 30)
        .unwrap();
}
#[test]
fn rec_005_policy_or_profiles_change_requires_new_view_and_denies_old_authority() {
    let mut config = SessionConfig::default();
    config.supported_profiles.insert("extra.profile.v1".into());
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    let mut hello = hello(3);
    hello.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: session.view_id,
        acknowledged_revision: 30,
    });
    let mut new_auth = auth("alice", false);
    new_auth.policy_version = 2;
    let changed = broker
        .negotiate(Some(new_auth.clone()), hello.clone(), 1)
        .unwrap();
    assert_ne!(changed.view_id, session.view_id);
    assert_eq!(
        changed.resume,
        ResumeDisposition::ResetRequired(ResetReason::ViewChanged)
    );
    assert_eq!(
        broker.authorize_projection(&session.client, session.view_id),
        Err(SessionError::WrongView)
    );
    assert_eq!(
        broker.validate_control(&session.client, &command(&session, &lease), 1),
        Err(Rejection::WrongView)
    );
    broker.take_cleanup().unwrap();
    usable(&mut broker, &changed, 30);
    hello.supported_profiles.insert("extra.profile.v1".into());
    hello.required_profiles.insert("extra.profile.v1".into());
    hello.resume.as_mut().unwrap().view_id = changed.view_id;
    let changed_profile = broker.negotiate(Some(new_auth), hello, 1).unwrap();
    assert_ne!(changed_profile.view_id, changed.view_id);
}
#[test]
fn another_principal_cannot_replace_client_id_or_revoke_its_lease() {
    let mut broker = broker();
    let alice = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &alice, 30);
    let lease = broker
        .acquire_control(&alice.client, alice.view_id, [1; 16], 0)
        .unwrap();
    assert!(matches!(
        broker.negotiate(Some(auth("mallory", true)), hello(3), 1),
        Err(SessionError::WrongPrincipal)
    ));
    assert_eq!(broker.current_lease(1), Some(lease));
    assert!(broker.take_cleanup().is_none());
    assert!(broker
        .authorize_projection(&alice.client, alice.view_id)
        .is_ok());
}
#[test]
fn revision_zero_regression_and_gap_cannot_enter_history() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    assert_eq!(
        broker.install_snapshot(&session.client, session.view_id, 0, 0),
        Err(SessionError::WrongRevision)
    );
    usable(&mut broker, &session, 30);
    assert_eq!(
        broker.install_snapshot(&session.client, session.view_id, 29, 1),
        Err(SessionError::WrongRevision)
    );
    assert_eq!(
        broker.publish(&session.client, session.view_id, 30, 32, vec![1], 0),
        Err(SessionError::WrongRevision)
    );
    assert_eq!(
        broker
            .queued_bytes(&session.client, session.view_id)
            .unwrap(),
        0
    );
}
#[test]
fn mul_004_slow_consumer_is_closed_without_blocking_healthy_client() {
    let mut broker = broker();
    let healthy = connect(&mut broker, 3, "alice", false);
    let mut hello = hello(4);
    hello.receiver_limits.outbound_queue_bytes = 2;
    let slow = broker
        .negotiate(Some(auth("bob", false)), hello, 0)
        .unwrap();
    usable(&mut broker, &healthy, 30);
    usable(&mut broker, &slow, 30);
    broker
        .publish(&slow.client, slow.view_id, 30, 31, vec![1; 2], 0)
        .unwrap();
    assert_eq!(
        broker.publish(&slow.client, slow.view_id, 31, 32, vec![2], 0),
        Err(SessionError::SlowConsumer)
    );
    assert_eq!(
        broker.authorize_projection(&slow.client, slow.view_id),
        Err(SessionError::WrongView)
    );
    for revision in 31..=35 {
        broker
            .publish(
                &healthy.client,
                healthy.view_id,
                revision - 1,
                revision,
                vec![revision as u8],
                0,
            )
            .unwrap();
        assert_eq!(
            broker
                .take_outbound(&healthy.client, healthy.view_id)
                .unwrap()
                .unwrap()[0],
            revision as u8
        );
    }
}
#[test]
fn queue_record_count_is_bounded_even_for_zero_byte_publications() {
    let mut broker = broker();
    let mut hello = hello(3);
    hello.receiver_limits.records = 2;
    let session = broker
        .negotiate(Some(auth("alice", false)), hello, 0)
        .unwrap();
    usable(&mut broker, &session, 30);
    broker
        .publish(&session.client, session.view_id, 30, 31, vec![], 0)
        .unwrap();
    broker
        .publish(&session.client, session.view_id, 31, 32, vec![], 0)
        .unwrap();
    assert_eq!(
        broker.publish(&session.client, session.view_id, 32, 33, vec![], 0),
        Err(SessionError::SlowConsumer)
    );
}
#[test]
fn history_queue_snapshot_and_outbound_refs_share_a_bounded_allocation_budget() {
    let config = SessionConfig {
        max_history_records: 1,
        ..SessionConfig::default()
    };
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let mut hello = hello(3);
    hello.receiver_limits.view_memory_bytes = 7 + 2 * PUBLICATION_OVERHEAD_BYTES;
    let session = broker
        .negotiate(Some(auth("alice", false)), hello, 0)
        .unwrap();
    usable(&mut broker, &session, 30);
    broker
        .publish(&session.client, session.view_id, 30, 31, vec![1; 3], 0)
        .unwrap();
    let inflight = broker
        .take_outbound(&session.client, session.view_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        4 + PUBLICATION_OVERHEAD_BYTES
    );
    broker
        .publish(&session.client, session.view_id, 31, 32, vec![2; 3], 0)
        .unwrap();
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        7 + 2 * PUBLICATION_OVERHEAD_BYTES
    );
    assert_eq!(
        broker.publish(&session.client, session.view_id, 32, 33, vec![3], 0),
        Err(SessionError::SlowConsumer)
    );
    assert_eq!(inflight.len(), 3);
    drop(inflight);
}
#[test]
fn control_request_retention_and_client_count_fail_closed() {
    let config = SessionConfig {
        max_clients: 1,
        max_control_requests: 1,
        ..SessionConfig::default()
    };
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    assert!(matches!(
        broker.negotiate(Some(auth("bob", true)), hello(4), 0),
        Err(SessionError::RetentionExhausted)
    ));
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    assert_eq!(
        broker.renew_control(
            &session.client,
            session.view_id,
            [2; 16],
            lease.lease_id,
            lease.generation,
            1
        ),
        Err(SessionError::RetentionExhausted)
    );
    broker
        .release_control(
            &session.client,
            [3; 16],
            lease.lease_id,
            lease.generation,
            1,
        )
        .unwrap();
    broker.take_cleanup().unwrap();
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 2),
        Err(SessionError::StaleRequest)
    );
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [4; 16], 2),
        Err(SessionError::RetentionExhausted)
    );
}

#[test]
fn empty_publications_charge_allocation_overhead_against_small_memory_ceiling() {
    let mut broker = broker();
    let mut hello = hello(3);
    hello.receiver_limits.view_memory_bytes = PUBLICATION_OVERHEAD_BYTES + 1;
    let session = broker
        .negotiate(Some(auth("alice", false)), hello, 0)
        .unwrap();
    usable(&mut broker, &session, 30);
    broker
        .publish(&session.client, session.view_id, 30, 31, vec![], 0)
        .unwrap();
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        PUBLICATION_OVERHEAD_BYTES + 1
    );
    assert_eq!(
        broker.publish(&session.client, session.view_id, 31, 32, vec![], 0),
        Err(SessionError::SlowConsumer)
    );
}
#[test]
fn existing_control_cannot_bypass_missing_resources_or_snapshot_presentation() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    let command = command(&session, &lease);
    broker
        .mark_resources_missing(&session.client, session.view_id)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Err(Rejection::Unauthorized)
    );
    broker
        .acknowledge_applied(&session.client, session.view_id, 30, true)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Err(Rejection::Unauthorized)
    );
    broker
        .acknowledge_presented(&session.client, session.view_id, 30)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Ok(())
    );
    broker
        .install_snapshot(&session.client, session.view_id, 31, 1)
        .unwrap();
    broker
        .acknowledge_applied(&session.client, session.view_id, 31, true)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Err(Rejection::Unauthorized)
    );
    broker
        .acknowledge_presented(&session.client, session.view_id, 31)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Ok(())
    );
}

#[test]
fn lowering_memory_ceiling_waits_for_inflight_payloads_to_drop() {
    let config = SessionConfig {
        max_history_records: 1,
        ..SessionConfig::default()
    };
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let session = connect(&mut broker, 3, "alice", false);
    usable(&mut broker, &session, 30);
    broker
        .publish(&session.client, session.view_id, 30, 31, vec![1; 3], 0)
        .unwrap();
    let inflight = broker
        .take_outbound(&session.client, session.view_id)
        .unwrap()
        .unwrap();
    broker
        .publish(&session.client, session.view_id, 31, 32, vec![], 0)
        .unwrap();
    broker.disconnect(&session.client, 1).unwrap();
    let mut lowered = hello(3);
    lowered.receiver_limits.view_memory_bytes = PUBLICATION_OVERHEAD_BYTES + 44;
    assert!(matches!(
        broker.negotiate(Some(auth("alice", false)), lowered.clone(), 2),
        Err(SessionError::Oversized)
    ));
    assert_eq!(inflight.len(), 3);
    drop(inflight);
    let reconnected = broker
        .negotiate(Some(auth("alice", false)), lowered, 2)
        .unwrap();
    assert_eq!(reconnected.view_id, session.view_id);
    assert_eq!(
        reconnected.limits.view_memory_bytes,
        PUBLICATION_OVERHEAD_BYTES + 44
    );
    assert_eq!(
        broker
            .retained_memory_bytes(&reconnected.client, reconnected.view_id)
            .unwrap(),
        PUBLICATION_OVERHEAD_BYTES + 1
    );
}

#[test]
fn snapshot_read_handles_are_charged_without_resetting_acknowledgements_or_history() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    broker
        .publish(&session.client, session.view_id, 30, 31, vec![1], 0)
        .unwrap();
    let before = broker
        .retained_memory_bytes(&session.client, session.view_id)
        .unwrap();
    let snapshot = broker
        .snapshot_payload(&session.client, session.view_id, vec![9; 4])
        .unwrap();
    let clone = snapshot.clone();
    drop(snapshot);
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        before + 4 + PUBLICATION_OVERHEAD_BYTES
    );
    drop(clone);
    assert_eq!(
        broker
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap(),
        before
    );
    broker.disconnect(&session.client, 1).unwrap();
    let mut hello = hello(3);
    hello.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: session.view_id,
        acknowledged_revision: 30,
    });
    let resumed = broker
        .negotiate(Some(auth("alice", true)), hello, 2)
        .unwrap();
    assert!(matches!(
        resumed.resume,
        ResumeDisposition::Accepted {
            from_revision: 30,
            through_revision: 31,
            ..
        }
    ));
}
#[test]
fn snapshot_read_handles_respect_envelope_and_total_memory_limits() {
    let mut broker = broker();
    let mut hello = hello(3);
    hello.receiver_limits.envelope_bytes = 8;
    hello.receiver_limits.view_memory_bytes = 1 + 4 + PUBLICATION_OVERHEAD_BYTES;
    let session = broker
        .negotiate(Some(auth("alice", false)), hello, 0)
        .unwrap();
    usable(&mut broker, &session, 30);
    assert_eq!(
        broker.snapshot_payload(&session.client, session.view_id, vec![1; 9]),
        Err(SessionError::Oversized)
    );
    let retained = broker
        .snapshot_payload(&session.client, session.view_id, vec![1; 4])
        .unwrap();
    assert_eq!(
        broker.snapshot_payload(&session.client, session.view_id, vec![1]),
        Err(SessionError::Oversized)
    );
    drop(retained);
    assert!(broker
        .snapshot_payload(&session.client, session.view_id, vec![1; 4])
        .is_ok());
}

#[test]
fn semantic_only_readiness_never_grants_raw_or_visual_authority() {
    let mut config = SessionConfig::default();
    config.supported_profiles.insert("semantics.only.v1".into());
    let mut broker = SessionBroker::new([1; 16], [2; 16], config).unwrap();
    let mut hello = hello(3);
    hello.supported_profiles.insert("semantics.only.v1".into());
    hello.required_profiles.insert("semantics.only.v1".into());
    let mut authorization = auth("semantic", true);
    authorization.permissions.actions.extend(
        ["RawInputBatch", "TextComposition", "ResizeRequest"]
            .into_iter()
            .map(str::to_owned),
    );
    let session = broker.negotiate(Some(authorization), hello, 0).unwrap();
    broker
        .install_snapshot(&session.client, session.view_id, 1, 1)
        .unwrap();
    assert_eq!(
        broker.acknowledge_semantic_ready(&session.client, session.view_id, 1),
        Err(SessionError::NotPresentable)
    );
    broker
        .acknowledge_applied(&session.client, session.view_id, 1, true)
        .unwrap();
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [1; 16], 0),
        Err(SessionError::Unauthorized)
    );
    assert_eq!(
        broker.acknowledge_presented(&session.client, session.view_id, 1),
        Err(SessionError::NotPresentable)
    );
    assert_eq!(
        broker.acknowledge_semantic_ready(&session.client, session.view_id, 0),
        Err(SessionError::NotPresentable)
    );
    broker
        .acknowledge_semantic_ready(&session.client, session.view_id, 1)
        .unwrap();
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    assert!(!lease.allowed_actions.contains("RawInputBatch"));
    let command = command(&session, &lease);
    assert_eq!(
        broker.validate_control(&session.client, &command, 0),
        Ok(())
    );
    for (family, operation) in [
        (CommandFamily::RawInput, "RawInputBatch"),
        (CommandFamily::TextComposition, "TextComposition"),
        (CommandFamily::Resize, "ResizeRequest"),
    ] {
        assert_eq!(
            broker.validate_family_control(
                &session.client,
                &command.header(),
                family,
                operation,
                0
            ),
            Err(Rejection::Unauthorized)
        );
    }
    broker
        .renew_control(
            &session.client,
            session.view_id,
            [2; 16],
            lease.lease_id,
            lease.generation,
            1,
        )
        .unwrap();
    broker
        .mark_resources_missing(&session.client, session.view_id)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Err(Rejection::Unauthorized)
    );
    broker
        .acknowledge_applied(&session.client, session.view_id, 1, true)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Err(Rejection::Unauthorized)
    );
    broker
        .acknowledge_semantic_ready(&session.client, session.view_id, 1)
        .unwrap();
    assert_eq!(
        broker.validate_control(&session.client, &command, 1),
        Ok(())
    );
    broker.disconnect(&session.client, 1).unwrap();
    assert!(broker.take_cleanup().is_some());
}

#[test]
fn publication_attachment_keeps_charge_and_cannot_cross_fresh_policy_view() {
    let mut broker = broker();
    let s = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &s, 1);
    let attachment = broker
        .reserve_view_memory(&s.client, s.view_id, 4096)
        .unwrap()
        .attach("SENSITIVE-ATTACHMENT-SENTINEL".to_owned());
    let weak = attachment.downgrade();
    broker
        .publish_with_attachment(
            &s.client,
            s.view_id,
            1,
            2,
            b"SENSITIVE-PAYLOAD-SENTINEL".to_vec(),
            Some(attachment.clone()),
            0,
        )
        .unwrap();
    let held = broker.take_outbound(&s.client, s.view_id).unwrap().unwrap();
    assert!(!format!("{held:?}").contains("SENSITIVE"));
    assert!(!format!("{held:?}").contains(&format!("{:?}", &held[..])));
    let mut changed = auth("alice", false);
    changed.policy_version = 2;
    let fresh = broker.negotiate(Some(changed), hello(3), 1).unwrap();
    assert_ne!(s.view_id, fresh.view_id);
    assert_eq!(
        broker.snapshot_publication_payload(
            &fresh.client,
            fresh.view_id,
            vec![0],
            Some(attachment.clone())
        ),
        Err(SessionError::WrongView)
    );
    assert!(
        broker
            .retained_memory_bytes(&fresh.client, fresh.view_id)
            .unwrap()
            >= 4096
    );
    drop(attachment);
    assert!(weak.upgrade().is_some());
    drop(held);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        broker
            .retained_memory_bytes(&fresh.client, fresh.view_id)
            .unwrap(),
        0
    );
}

#[test]
fn snapshot_chunk_producer_bound_does_not_widen_single_envelope_api() {
    let mut broker = broker();
    let mut h = hello(3);
    h.receiver_limits.envelope_bytes = 8;
    h.receiver_limits.snapshot_bytes = 32;
    let s = broker.negotiate(Some(auth("alice", true)), h, 0).unwrap();
    assert_eq!(
        broker.snapshot_payload(&s.client, s.view_id, vec![1; 9]),
        Err(SessionError::Oversized)
    );
    let output = broker
        .snapshot_publication_payload(&s.client, s.view_id, vec![1; 32], None)
        .unwrap();
    assert_eq!(output.len(), 32);
    assert_eq!(
        broker.snapshot_publication_payload(&s.client, s.view_id, vec![1; 33], None),
        Err(SessionError::Oversized)
    );
}

#[test]
fn retained_transaction_ack_survives_history_eviction_but_rejects_forged_scope() {
    let mut broker = SessionBroker::new(
        [1; 16],
        [2; 16],
        SessionConfig {
            max_history_records: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let s = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &s, 1);
    broker
        .publish(&s.client, s.view_id, 1, 2, vec![2], 0)
        .unwrap();
    let retained = broker.take_outbound(&s.client, s.view_id).unwrap().unwrap();
    assert_eq!(retained.publication_revision(), Some(2));
    broker
        .publish(&s.client, s.view_id, 2, 3, vec![3], 0)
        .unwrap();
    assert_eq!(
        broker.acknowledge_applied(&s.client, s.view_id, 2, true),
        Err(SessionError::WrongRevision)
    );
    assert_eq!(
        broker.acknowledge_applied_publication(&s.client, s.view_id, 3, true, &retained),
        Err(SessionError::WrongRevision)
    );
    // Read-only arbitrary bytes and adapter allocations carry no published
    // transaction stamp, even if their contents equal a real transaction.
    let arbitrary = broker
        .snapshot_payload(&s.client, s.view_id, vec![2])
        .unwrap();
    assert_eq!(arbitrary.publication_revision(), None);
    assert_eq!(
        broker.acknowledge_applied_publication(&s.client, s.view_id, 2, true, &arbitrary),
        Err(SessionError::WrongRevision)
    );
    let other = connect(&mut broker, 4, "bob", true);
    usable(&mut broker, &other, 1);
    assert_eq!(
        broker.acknowledge_applied_publication(&other.client, other.view_id, 2, true, &retained),
        Err(SessionError::WrongView)
    );
    // Even identically chosen session/view IDs from another broker do not share
    // its private budget identity.
    let mut independent = SessionBroker::new(
        [1; 16],
        [2; 16],
        SessionConfig {
            max_history_records: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let clone_scope = connect(&mut independent, 3, "alice", true);
    usable(&mut independent, &clone_scope, 1);
    assert_eq!(clone_scope.view_id, s.view_id);
    assert_eq!(
        independent.acknowledge_applied_publication(
            &clone_scope.client,
            clone_scope.view_id,
            2,
            true,
            &retained
        ),
        Err(SessionError::WrongView)
    );
    broker
        .acknowledge_applied_publication(&s.client, s.view_id, 2, true, &retained)
        .unwrap();
    // An explicit checkpoint invalidates retained proof from its former chain.
    broker.install_snapshot(&s.client, s.view_id, 3, 1).unwrap();
    assert_eq!(
        broker.acknowledge_applied_publication(&s.client, s.view_id, 2, true, &retained),
        Err(SessionError::WrongRevision)
    );
    let mut changed = auth("alice", true);
    changed.policy_version = 2;
    let fresh = broker.negotiate(Some(changed), hello(3), 1).unwrap();
    assert_ne!(fresh.view_id, s.view_id);
    assert_eq!(
        broker.acknowledge_applied_publication(&fresh.client, fresh.view_id, 2, true, &retained),
        Err(SessionError::WrongView)
    );
}

#[test]
fn renewal_during_resource_wait_preserves_scope_without_authorizing_input() {
    let mut broker = broker();
    let session = connect(&mut broker, 3, "alice", true);
    usable(&mut broker, &session, 30);
    let lease = broker
        .acquire_control(&session.client, session.view_id, [1; 16], 0)
        .unwrap();
    broker
        .mark_resources_missing(&session.client, session.view_id)
        .unwrap();
    let renewed = broker
        .renew_control(
            &session.client,
            session.view_id,
            [2; 16],
            lease.lease_id,
            lease.generation,
            2000,
        )
        .unwrap();
    assert_eq!(renewed.generation, lease.generation);
    assert_eq!(renewed.expires_at_ms, 7000);
    assert_eq!(
        broker.acquire_control(&session.client, session.view_id, [3; 16], 2001),
        Err(SessionError::Unauthorized)
    );
    assert_eq!(
        broker.record_button(&renewed, 1, true, 2001),
        Err(SessionError::Unauthorized)
    );
    broker.revoke_control(2002);
    assert_eq!(
        broker.renew_control(
            &session.client,
            session.view_id,
            [4; 16],
            lease.lease_id,
            lease.generation,
            2003
        ),
        Err(SessionError::StaleControl)
    );
}
