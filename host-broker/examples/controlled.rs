use prost::Message;
use rui_host_broker::{ControlledHost, HostConfig, COUNTER_ID, READ_SECRET_PERMISSION};
use rui_protocol_core::receiver::{ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{ResourceContext, ResourceScope};
use rui_protocol_core::session::{
    ClientHello, NegotiatedSession, Permissions, ReceiverLimits, SessionConfig,
    VerifiedAuthorization,
};
use rui_protocol_core::wire::proto as p;
use std::collections::BTreeSet;

fn connect(
    host: &mut ControlledHost,
    id: u8,
    controller: bool,
) -> Result<(NegotiatedSession, ViewReceiver), Box<dyn std::error::Error>> {
    let auth = VerifiedAuthorization {
        principal: if controller { "controller" } else { "observer" }.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: controller,
            actions: if controller {
                ["Activate", "ReplaceText", READ_SECRET_PERMISSION]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                BTreeSet::new()
            },
        },
    };
    let hello = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    let session = host.negotiate(auth, hello, 0)?;
    let context = ResourceContext {
        scope: ResourceScope {
            session_id: session.session_id,
            epoch: session.epoch,
            principal_id: [id; 16],
        },
        view_id: session.view_id,
    };
    let mut receiver =
        ViewReceiver::new(context, session.limits.clone(), ReceiverConfig::default())?;
    let snapshot = host.checkpoint(&session.client, session.view_id)?;
    receiver.receive_envelope([id; 16], &snapshot)?;
    host.acknowledge_applied(&session.client, session.view_id, 1)?;
    // This headless fixture simulates the trusted presentation completion
    // callback. It does not claim that a GUI renderer presented pixels.
    let presented = receiver.mark_presented(1)?;
    host.acknowledge_presented(&session.client, session.view_id, presented.revision)?;
    Ok((session, receiver))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = ControlledHost::new(
        HostConfig {
            max_text_bytes: 64,
            ..HostConfig::default()
        },
        "fixture-private-value".into(),
    )?;
    let (controller, mut controller_receiver) = connect(&mut host, 3, true)?;
    let (observer, mut observer_receiver) = connect(&mut host, 4, false)?;
    let lease = host.acquire_control(&controller.client, controller.view_id, [1; 16], 0)?;
    let command = p::SemanticCommand {
        epoch: controller.epoch.to_vec(),
        view_id: controller.view_id.to_vec(),
        client_instance_id: controller.client.client_instance_id.to_vec(),
        command_seq: 1,
        control_lease_id: lease.lease_id.to_vec(),
        control_generation: lease.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 1,
        preconditions: Some(p::Preconditions {
            target_version: Some(1),
            ..Default::default()
        }),
        arguments: Some(p::semantic_command::Arguments::NoArguments(
            p::EmptyArguments {},
        )),
    }
    .encode_to_vec();
    let result = host.submit(&controller.client, &command, 1)?;
    assert_eq!(host.submit(&controller.client, &command, 2)?, result);
    let delta = host
        .take_outbound(&controller.client, controller.view_id)?
        .ok_or("missing controller update")?;
    controller_receiver.receive_envelope([10; 16], &delta)?;
    let delta = host
        .take_outbound(&observer.client, observer.view_id)?
        .ok_or("missing observer update")?;
    observer_receiver.receive_envelope([11; 16], &delta)?;
    assert_eq!(controller_receiver.state()?.revision, 2);
    assert_eq!(observer_receiver.state()?.revision, 2);
    println!("{:?}", host.diagnostics());
    Ok(())
}
