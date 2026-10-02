mod common;
use common::*;
use rui_client_desktop::*;
use rui_protocol_core::{model::*, resources::*};

fn fixture(appearance: CursorAppearance) -> State {
    let mut state = state([solid(
        1,
        rect(0.0, 0.0, 40.0, 40.0),
        color(1.0, 0.0, 0.0, 1.0),
    )]);
    state
        .records
        .insert(10, Record::Surface(surface(10, 1, 0.0, 0.0, 40.0, 40.0)));
    state.records.insert(
        20,
        Record::Session(SessionState {
            id: 20,
            version: 1,
            canvas: rect(0.0, 0.0, 40.0, 40.0),
            cursor: Some(Cursor {
                position: point(0.0, 0.0),
                hotspot: point(0.0, 0.0),
                appearance,
            }),
            focus: None,
            focus_surface: None,
            focus_generation: 0,
        }),
    );
    state
}
fn renderer() -> Renderer {
    Renderer::new_with_profile(
        RenderLimits::default(),
        MemoryBudget::new(128 << 20),
        RenderProfile {
            cursor_vectors: true,
        },
    )
    .unwrap()
}
fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
    frame.pixels()[y * frame.viewport.width as usize + x].rgba8()
}
#[test]
fn bundled_shapes_paint_defined_white_interiors_above_red_surface() {
    for (shape, x, y) in [
        (CursorShape::Default, 3, 6),
        (CursorShape::Pointer, 8, 5),
        (CursorShape::Text, 9, 10),
    ] {
        let s = fixture(CursorAppearance::Shape(shape));
        let frame = renderer()
            .render_view(&s, Viewport::new(40, 40), &())
            .unwrap();
        assert_eq!(pixel(&frame, x, y), [255, 255, 255, 255], "{shape:?}");
        assert_eq!(pixel(&frame, 35, 35), [255, 0, 0, 255]);
    }
}
#[test]
fn visible_cursor_requires_explicit_profile_and_hidden_is_identical_to_no_cursor() {
    let mut s = fixture(CursorAppearance::Shape(CursorShape::Default));
    let ordinary = Renderer::new(RenderLimits::default(), MemoryBudget::new(128 << 20)).unwrap();
    assert!(matches!(
        ordinary.render_view(&s, Viewport::new(40, 40), &()),
        Err(RenderError::UnsupportedCursor)
    ));
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.cursor.as_mut().unwrap().appearance = CursorAppearance::Shape(CursorShape::Hidden);
    let hidden = renderer()
        .render_view(&s, Viewport::new(40, 40), &())
        .unwrap();
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.cursor = None;
    let absent = renderer()
        .render_view(&s, Viewport::new(40, 40), &())
        .unwrap();
    assert_eq!(hidden.pixels(), absent.pixels());
}
#[test]
fn custom_outline_hotspot_and_observer_zoom_use_authoritative_desktop_mapping() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 0.0),
            Command::Line(2.0, 0.0),
            Command::Line(2.0, 2.0),
            Command::Line(0.0, 2.0),
            Command::Close,
        ],
    );
    let mut s = fixture(CursorAppearance::Outline(id));
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.cursor.as_mut().unwrap().position = point(7.0, 9.0);
    session.cursor.as_mut().unwrap().hotspot = point(2.0, 3.0);
    let viewport = Viewport {
        width: 8,
        height: 8,
        scale: 2.0,
        origin: point(4.0, 5.0),
    };
    let frame = renderer().render_view(&s, viewport, &resources).unwrap();
    assert_eq!(pixel(&frame, 2, 2), [0, 0, 0, 255]);
    assert_eq!(pixel(&frame, 5, 5), [0, 0, 0, 255]);
    assert_eq!(pixel(&frame, 1, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&frame, 6, 5), [255, 0, 0, 255]);
    assert!(s.resource_refs().contains(&id));
}
#[test]
fn cursor_respects_session_canvas_and_custom_closure_errors() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 0.0),
            Command::Line(4.0, 0.0),
            Command::Line(4.0, 4.0),
            Command::Line(0.0, 4.0),
            Command::Close,
        ],
    );
    let mut s = fixture(CursorAppearance::Outline(id));
    assert!(
        matches!(renderer().render_view(&s,Viewport::new(8,8),&()),Err(RenderError::MissingResource(missing)) if missing==id)
    );
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.canvas = rect(0.0, 0.0, 2.0, 2.0);
    let frame = renderer()
        .render_view(&s, Viewport::new(8, 8), &resources)
        .unwrap();
    assert_eq!(pixel(&frame, 1, 1), [0, 0, 0, 255]);
    assert_eq!(pixel(&frame, 2, 1), [0, 0, 0, 0]);
    let wrong = resources.add(
        ResourceKind::GlyphOutline,
        &[
            Command::Move(0.0, 0.0),
            Command::Line(1.0, 0.0),
            Command::Line(1.0, 1.0),
            Command::Close,
        ],
    );
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.cursor.as_mut().unwrap().appearance = CursorAppearance::Outline(wrong);
    assert!(matches!(
        renderer().render_view(&s, Viewport::new(8, 8), &resources),
        Err(RenderError::ResourceDescriptor)
    ));
}
#[test]
fn malformed_cursor_and_work_limits_release_every_allocation() {
    let mut s = fixture(CursorAppearance::Shape(CursorShape::Pointer));
    let budget = MemoryBudget::new(128 << 20);
    let limits = RenderLimits {
        max_work: 1,
        ..RenderLimits::default()
    };
    let renderer = Renderer::new_with_profile(
        limits,
        budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )
    .unwrap();
    assert!(matches!(
        renderer.render_view(&s, Viewport::new(8, 8), &()),
        Err(RenderError::Limit(_))
    ));
    assert_eq!(budget.used(), 0);
    let Record::Session(session) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    session.cursor.as_mut().unwrap().hotspot.x = f64::NAN;
    assert!(matches!(
        self::renderer().render_view(&s, Viewport::new(8, 8), &()),
        Err(RenderError::Invalid)
    ));
}
