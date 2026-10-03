use prost::Message;
use rui_protocol_core::{
    commands::{CommandLedger, CommandResult, LedgerLimits},
    model::{Affine, Color, Point, Rect, Size, SurfaceRole},
    resources::MemoryBudget,
    session::*,
    wire::proto as pb,
};
use rui_session_shell::{
    backend::{Backend, DEVICE, SEAT},
    topology::WindowSpec,
    Framebuffer, InjectionScope, RegionGrant,
};
use std::time::{Duration, Instant};
pub struct Fixture {
    pub backend: Backend,
    pub broker: SessionBroker,
    pub ledger: CommandLedger,
    pub session: NegotiatedSession,
    pub lease: ControlLease,
    pub main: u64,
    pub seq: u64,
    pub revision: u64,
    pub budget: MemoryBudget,
}
pub fn window(role: SurfaceRole, x: f64, y: f64, width: f64, height: f64, z: i64) -> WindowSpec {
    WindowSpec {
        role,
        parent: None,
        modal_for: None,
        desktop_rect: Rect {
            x,
            y,
            width,
            height,
        },
        content_size: Size { width, height },
        transform: Affine {
            xx: 1.0,
            yy: 1.0,
            xy: 0.0,
            yx: 0.0,
            tx: 0.0,
            ty: 0.0,
        },
        stacking_order: z,
        color: Color {
            red: 0.0,
            green: 0.0,
            blue: 1.0,
            alpha: 1.0,
        },
    }
}
impl Fixture {
    pub fn new() -> Self {
        let budget = MemoryBudget::new(128 << 20);
        let grant = RegionGrant::new(
            InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 120.0,
            },
            Rect {
                x: 40.0,
                y: 80.0,
                width: 600.0,
                height: 240.0,
            },
            Framebuffer {
                width: 1000,
                height: 700,
            },
            2.0,
        )
        .unwrap();
        let mut backend = Backend::new(grant, "de", &budget).unwrap();
        let main = backend
            .topology_mut()
            .open(window(SurfaceRole::Toplevel, 10.0, 10.0, 120.0, 90.0, 0))
            .unwrap();
        backend.topology_mut().focus(main).unwrap();
        let revision = backend.topology().revision();
        let config = SessionConfig::default();
        let mut broker = SessionBroker::new([1; 16], [2; 16], config.clone()).unwrap();
        let session = broker
            .negotiate(
                Some(VerifiedAuthorization {
                    principal: "isolated-native-test".into(),
                    policy_version: 1,
                    permissions: Permissions {
                        can_view: true,
                        can_control: true,
                        actions: ["RawInputBatch".into()].into_iter().collect(),
                    },
                }),
                ClientHello {
                    client_instance_id: [3; 16],
                    supported_versions: [1].into_iter().collect(),
                    supported_profiles: config.supported_profiles,
                    required_profiles: config.required_profiles,
                    receiver_limits: ReceiverLimits::default(),
                    resume: None,
                },
                0,
            )
            .unwrap();
        // Explicit trusted readiness fixture for admission tests. Native source
        // delivery is verified below; this is not a desktop Presented claim.
        broker
            .install_snapshot(&session.client, session.view_id, revision, 1)
            .unwrap();
        broker
            .acknowledge_applied(&session.client, session.view_id, revision, true)
            .unwrap();
        broker
            .acknowledge_presented(&session.client, session.view_id, revision)
            .unwrap();
        let lease = broker
            .acquire_control(&session.client, session.view_id, [9; 16], 0)
            .unwrap();
        Self {
            backend,
            broker,
            ledger: CommandLedger::new([2; 16], LedgerLimits::default()).unwrap(),
            session,
            lease,
            main,
            seq: 0,
            revision,
            budget,
        }
    }
    pub fn raw(&mut self, surface: u64, events: Vec<pb::raw_event::Event>) -> Vec<u8> {
        self.seq += 1;
        let route = self
            .backend
            .topology()
            .route(surface, self.revision)
            .unwrap();
        pb::RawInputBatch {
            header: Some(pb::CommandHeader {
                epoch: self.session.epoch.to_vec(),
                view_id: self.session.view_id.to_vec(),
                client_instance_id: self.session.client.client_instance_id.to_vec(),
                command_seq: self.seq,
                control_lease_id: self.lease.lease_id.to_vec(),
                control_generation: self.lease.generation,
                observed_view_revision: self.revision,
            }),
            surface_id: surface,
            routing_token: route.token.to_vec(),
            presented_revision: self.revision,
            seat_id: SEAT.into(),
            device_id: DEVICE.into(),
            key_namespace: "linux.evdev".into(),
            events: events
                .into_iter()
                .enumerate()
                .map(|(i, event)| pb::RawEvent {
                    timestamp_us: self.seq * 1000 + i as u64,
                    event: Some(event),
                })
                .collect(),
        }
        .encode_to_vec()
    }
    pub fn submit(&mut self, raw: &[u8], now: u64) -> CommandResult {
        self.backend
            .submit_raw(
                &mut self.broker,
                &mut self.ledger,
                &self.session.client,
                raw,
                now,
            )
            .unwrap()
    }
    pub fn pump(&mut self, now: u64) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.backend.pending_frames() != 0 {
            self.backend.poll(&mut self.broker, now).unwrap();
            assert!(Instant::now() < deadline, "native frame timeout");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
pub fn key(code: u32, pressed: bool) -> pb::raw_event::Event {
    pb::raw_event::Event::Key(pb::KeyEvent { code, pressed })
}
pub fn motion(x: f64, y: f64) -> pb::raw_event::Event {
    pb::raw_event::Event::Motion(pb::Point { x, y })
}
pub fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}
