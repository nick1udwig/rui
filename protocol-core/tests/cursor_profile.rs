use rui_protocol_core::{session::*, wire};
#[test]
fn required_vector_cursor_profile_is_explicit_and_rejects_older_clients() {
    let mut producer = SessionConfig::default();
    assert!(!producer.supported_profiles.contains("cursor.vector.v1"));
    producer
        .supported_profiles
        .insert("cursor.vector.v1".into());
    producer.required_profiles.insert("cursor.vector.v1".into());
    let mut broker = SessionBroker::new([1; 16], [2; 16], producer).unwrap();
    let authorization = VerifiedAuthorization {
        principal: "viewer".into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: false,
            actions: Default::default(),
        },
    };
    let mut hello = ClientHello {
        client_instance_id: [3; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    assert!(matches!(
        broker.negotiate(Some(authorization.clone()), hello.clone(), 0),
        Err(SessionError::UnsupportedProfile)
    ));
    hello.supported_profiles.insert("cursor.vector.v1".into());
    let session = broker.negotiate(Some(authorization), hello, 1).unwrap();
    assert!(session.profiles.contains("cursor.vector.v1"));
    wire::required_features(&session.profiles.into_iter().collect::<Vec<_>>()).unwrap();
}
