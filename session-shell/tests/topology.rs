use prost::Message;
use rui_client_desktop::{RenderLimits, RenderProfile, Renderer, Viewport};
use rui_protocol_core::{model::*, resources::MemoryBudget, wire::proto as pb};
use rui_session_shell::{
    topology::{Topology, WindowSpec},
    Framebuffer, InjectionScope, RegionGrant,
};
fn grant() -> RegionGrant {
    RegionGrant::new(
        InjectionScope::IsolatedWholeSession,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 80.0,
        },
        Rect {
            x: 30.0,
            y: 70.0,
            width: 300.0,
            height: 160.0,
        },
        Framebuffer {
            width: 1900,
            height: 1200,
        },
        2.0,
    )
    .unwrap()
}
fn spec(x: f64, y: f64, z: i64, color: Color) -> WindowSpec {
    WindowSpec {
        role: SurfaceRole::Toplevel,
        parent: None,
        modal_for: None,
        desktop_rect: Rect {
            x,
            y,
            width: 20.0,
            height: 20.0,
        },
        content_size: Size {
            width: 20.0,
            height: 20.0,
        },
        transform: Affine {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            tx: 0.0,
            ty: 0.0,
        },
        stacking_order: z,
        color,
    }
}
fn color(red: f64, green: f64, blue: f64) -> Color {
    Color {
        red,
        green,
        blue,
        alpha: 1.0,
    }
}
fn position(x: f64, y: f64) -> Point {
    Point { x, y }
}
#[test]
fn explicit_regions_reject_app_scope_and_framebuffer_inference() {
    let g = grant();
    assert_eq!(g.map(position(50.0, 40.0)).unwrap(), position(180.0, 150.0));
    assert_eq!(
        g.unmap(position(180.0, 150.0)).unwrap(),
        position(50.0, 40.0)
    );
    assert!(g.map(position(100.0, 40.0)).is_err());
    assert!(g.unmap(position(29.0, 70.0)).is_err());
    assert!(RegionGrant::new(
        InjectionScope::ApplicationOnly,
        g.logical(),
        g.granted(),
        g.framebuffer(),
        2.0
    )
    .is_err());
    assert!(RegionGrant::new(
        InjectionScope::IsolatedWholeSession,
        g.logical(),
        g.granted(),
        g.framebuffer(),
        f64::NAN
    )
    .is_err());
}
#[test]
fn source_transform_clip_stack_parent_and_modal_match_pixels_and_hit_testing() {
    let budget = MemoryBudget::new(128 << 20);
    let mut topology = Topology::new(grant(), &budget).unwrap();
    topology.set_cursor(CursorShape::Hidden).unwrap();
    let main = topology
        .open(spec(10.0, 10.0, 0, color(0.0, 0.0, 1.0)))
        .unwrap();
    topology.focus(main).unwrap();
    let mut popup = spec(25.0, 15.0, 10, color(1.0, 0.0, 0.0));
    popup.role = SurfaceRole::Popup;
    popup.parent = Some(main);
    popup.content_size = Size {
        width: 5.0,
        height: 5.0,
    };
    popup.transform.xx = 2.0;
    popup.transform.yy = 2.0;
    popup.desktop_rect.width = 99.0;
    let child = topology.open(popup).unwrap();
    let source = topology.snapshot().unwrap();
    let frame = Renderer::new_with_profile(
        RenderLimits::default(),
        budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )
    .unwrap()
    .render_view(source.state(), Viewport::new(100, 80), &())
    .unwrap();
    let pixel = |x: usize, y: usize| frame.pixels()[y * 100 + x].rgba8();
    assert_eq!(pixel(26, 23), [255, 0, 0, 255]);
    assert_eq!(pixel(29, 26), [0, 0, 255, 255]);
    assert_eq!(pixel(36, 23), [0, 0, 0, 0]);
    assert_eq!(topology.hit_test(position(26.0, 23.0)), Some(child));
    assert_eq!(topology.hit_test(position(29.0, 26.0)), Some(main));
    let mut tooltip = spec(25.0, 15.0, 20, color(0.0, 1.0, 0.0));
    tooltip.role = SurfaceRole::Tooltip;
    tooltip.parent = Some(main);
    let tip = topology.open(tooltip).unwrap();
    assert!(topology.focus(tip).is_err());
    assert_eq!(topology.hit_test(position(26.0, 23.0)), Some(child));
    let mut modal = spec(40.0, 20.0, 30, color(1.0, 1.0, 0.0));
    modal.role = SurfaceRole::Popup;
    modal.parent = Some(main);
    modal.modal_for = Some(main);
    let modal = topology.open(modal).unwrap();
    assert!(!topology.modal_allows(main));
    topology.focus(modal).unwrap();
    assert!(topology.focus(main).is_err());
    topology.close(modal).unwrap();
    assert_eq!(topology.focused_surface(), Some(main));
    topology.set_mapped(main, false).unwrap();
    assert_eq!(topology.focused_surface(), None);
    assert_eq!(topology.hit_test(position(26.0, 23.0)), None);
    assert!(!topology.route(child, 1).unwrap().mapped);
}
fn id(record: &pb::Record) -> u64 {
    match record.record.as_ref().unwrap() {
        pb::record::Record::Surface(r) => r.id,
        pb::record::Record::Scene(r) => r.id,
        pb::record::Record::Semantic(r) => r.id,
        pb::record::Record::Binding(r) => r.id,
        pb::record::Record::Session(r) => r.id,
        _ => panic!(),
    }
}
#[test]
fn actual_source_records_replay_cursor_and_activity_without_hidden_surface_changes() {
    let budget = MemoryBudget::new(128 << 20);
    let mut t = Topology::new(grant(), &budget).unwrap();
    let main = t.open(spec(10.0, 10.0, 0, color(0.0, 0.0, 1.0))).unwrap();
    let other = t.open(spec(60.0, 10.0, 1, color(1.0, 0.0, 0.0))).unwrap();
    t.focus(main).unwrap();
    let mut old = t.snapshot().unwrap();
    let mut replica = Replica::new(Limits::default());
    let snap = pb::Snapshot {
        revision: old.state().revision,
        records: old.records().to_vec(),
        ..Default::default()
    };
    replica
        .apply_snapshot(&snap.encode_to_vec(), &Default::default())
        .unwrap();
    for step in 0..3 {
        if step == 0 {
            t.source_pointer(grant().map(position(50.0, 60.0)).unwrap())
                .unwrap();
        } else {
            t.source_activity(main, "x").unwrap();
        }
        let new = t.snapshot().unwrap();
        let mutations = new
            .records()
            .iter()
            .filter(|r| old.records().iter().find(|prior| id(prior) == id(r)) != Some(*r))
            .cloned()
            .map(|r| pb::Mutation {
                mutation: Some(pb::mutation::Mutation::Put(r)),
            })
            .collect();
        let transaction = pb::Transaction {
            base_revision: old.state().revision,
            new_revision: new.state().revision,
            mutations,
            ..Default::default()
        };
        replica
            .apply_transaction(&transaction.encode_to_vec(), &Default::default())
            .unwrap();
        assert_eq!(serde_like(replica.state()), serde_like(new.state()));
        assert_eq!(
            old.records().iter().find(|r| id(r) == other),
            new.records().iter().find(|r| id(r) == other)
        );
        old = new;
    }
}
fn serde_like(state: &State) -> String {
    format!("{state:?}")
}
#[test]
fn configure_is_atomic_invalidates_routes_and_closed_incarnations_never_return() {
    let budget = MemoryBudget::new(128 << 20);
    let mut t = Topology::new(grant(), &budget).unwrap();
    let main = t.open(spec(10.0, 10.0, 0, color(0.0, 0.0, 1.0))).unwrap();
    let token = t.route(main, 1).unwrap().token;
    let rev = t.revision();
    let mut invalid = spec(20.0, 20.0, 1, color(1.0, 0.0, 0.0));
    invalid.transform.xx = f64::NAN;
    assert!(t
        .configure(
            main,
            invalid.desktop_rect,
            invalid.content_size,
            invalid.transform,
            1
        )
        .is_err());
    assert_eq!(t.revision(), rev);
    assert_eq!(t.route(main, 1).unwrap().token, token);
    let mut moved = spec(20.0, 20.0, 1, color(1.0, 0.0, 0.0));
    moved.transform.xx = 1.5;
    moved.transform.yy = 1.5;
    t.configure(
        main,
        moved.desktop_rect,
        moved.content_size,
        moved.transform,
        1,
    )
    .unwrap();
    assert_ne!(t.route(main, 1).unwrap().token, token);
    assert_eq!(
        t.native_point(main, position(2.0, 4.0)).unwrap(),
        grant().map(position(23.0, 26.0)).unwrap()
    );
    t.close(main).unwrap();
    let again = t.open(spec(10.0, 10.0, 0, color(0.0, 0.0, 1.0))).unwrap();
    assert_ne!(again, main);
    let s = t.snapshot().unwrap();
    for id in main..main + 6 {
        assert!(s.state().retired.contains(&id));
    }
}
#[test]
fn charged_projection_survives_source_drop_and_pressure_rejects_before_allocation() {
    let budget = MemoryBudget::new(8 << 20);
    let t = Topology::new(grant(), &budget).unwrap();
    let projection = t.snapshot().unwrap();
    let charged = budget.used();
    drop(t);
    assert!(budget.used() > 0);
    assert!(budget.used() < charged);
    drop(projection);
    assert_eq!(budget.used(), 0);
    let t = Topology::new(grant(), &budget).unwrap();
    let hold = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(t.snapshot().is_err());
    drop(hold);
    assert!(t.snapshot().is_ok());
}
