//! Optional local frontend for the owned vector session. Native GDK physical
//! events enter only the private local-device capability, before source work.
use crate::actor::{LocalDevicePort, LocalEvent, Pending, Response};
use crate::ShellError;
use gtk4::{gdk, glib, graphene, prelude::*, subclass::prelude::*};
use prost::Message;
use rui_client_desktop::{RenderLimits, RenderProfile, Renderer, Viewport};
use rui_protocol_core::{
    input::inverse_pointer,
    model::{Affine, Color, Point, Record, Rect, TypedValue},
    resources::{MemoryBudget, MemoryReservation},
    wire::proto as pb,
};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Painted {
    pub revision: u64,
    pub source_sha256: [u8; 32],
    pub rgba8_sha256: [u8; 32],
    pub canvas: Rect,
    pub source_width: u32,
    pub source_height: u32,
}
mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Canvas {
        pub texture: RefCell<Option<gdk::MemoryTexture>>,
        pub stamp: Cell<Option<Painted>>,
        pub submitted: Cell<Option<Painted>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "RuiIsolatedSourceCanvas";
        type Type = super::Canvas;
        type ParentType = gtk4::Widget;
    }
    impl ObjectImpl for Canvas {}
    impl WidgetImpl for Canvas {
        fn measure(&self, orientation: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (
                64,
                if orientation == gtk4::Orientation::Horizontal {
                    600
                } else {
                    360
                },
                -1,
                -1,
            )
        }
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32),
            );
            if let (Some(texture), Some(stamp)) = (self.texture.borrow().as_ref(), self.stamp.get())
            {
                if let Some((_, clip)) = mapping(widget.width(), widget.height(), stamp.canvas) {
                    snapshot.append_texture(
                        texture,
                        &graphene::Rect::new(
                            clip.x as f32,
                            clip.y as f32,
                            clip.width as f32,
                            clip.height as f32,
                        ),
                    );
                    self.submitted.set(Some(stamp));
                }
            }
        }
    }
}
glib::wrapper! { pub struct Canvas(ObjectSubclass<imp::Canvas>) @extends gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget; }

/// The local viewport scales uniformly and letterboxes. Its allocation never
/// changes source geometry or the independent EIS granted-region transform.
fn mapping(width: i32, height: i32, canvas: Rect) -> Option<(Affine, Rect)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let scale = (width as f64 / canvas.width).min(height as f64 / canvas.height);
    let clip = Rect {
        x: (width as f64 - canvas.width * scale) / 2.0,
        y: (height as f64 - canvas.height * scale) / 2.0,
        width: canvas.width * scale,
        height: canvas.height * scale,
    };
    Some((
        Affine {
            xx: scale,
            yy: scale,
            xy: 0.0,
            yx: 0.0,
            tx: clip.x - canvas.x * scale,
            ty: clip.y - canvas.y * scale,
        },
        clip,
    ))
}
impl Canvas {
    pub fn desktop_point(&self, x: f64, y: f64) -> Result<Point, ShellError> {
        let stamp = self.imp().stamp.get().ok_or(ShellError::Geometry)?;
        let (transform, clip) =
            mapping(self.width(), self.height(), stamp.canvas).ok_or(ShellError::Geometry)?;
        Ok(inverse_pointer(
            Point { x, y },
            transform,
            clip,
            stamp.canvas,
        )?)
    }
}
#[derive(Default)]
struct Counters {
    pressed: Cell<u64>,
    released: Cell<u64>,
    pointer: Cell<u64>,
}
pub struct SourceWindow {
    pub window: gtk4::ApplicationWindow,
    pub canvas: Canvas,
    painted: Rc<Cell<Option<Painted>>>,
    counters: Rc<Counters>,
    failure: Rc<Cell<Option<&'static str>>>,
    timer: Option<glib::SourceId>,
    local: LocalDevicePort,
    controllers: Vec<gtk4::EventController>,
    clock_hook: Rc<RefCell<Option<(gdk::FrameClock, glib::SignalHandlerId)>>>,
    _native: Rc<MemoryReservation>,
}
impl SourceWindow {
    /// Observation is opt-in and contains fixed-size metadata only. This is a
    /// source-window GTK after-paint opportunity, never a remote PresentedAck.
    pub fn painted(&self) -> Option<Painted> {
        self.painted.get()
    }
    pub fn native_callbacks(&self) -> [u64; 3] {
        [
            self.counters.pressed.get(),
            self.counters.released.get(),
            self.counters.pointer.get(),
        ]
    }
    pub fn failure(&self) -> Option<&'static str> {
        self.failure.get()
    }
}
impl Drop for SourceWindow {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        let _ = self.local.cancel_input();
        if let Some((clock, handler)) = self.clock_hook.borrow_mut().take() {
            clock.disconnect(handler);
        }
        for controller in &self.controllers {
            self.canvas.remove_controller(controller);
        }
        self.canvas.imp().texture.replace(None);
        self.canvas.imp().stamp.set(None);
        self.canvas.imp().submitted.set(None);
        self.window.set_child(None::<&gtk4::Widget>);
        self.window.destroy();
    }
}
fn increment(counter: &Cell<u64>) {
    counter.set(counter.get().saturating_add(1));
}
fn reject(failure: &Cell<Option<&'static str>>, label: &gtk4::Label, message: &'static str) {
    if failure.get().is_none() {
        failure.set(Some(message));
    }
    label.set_text(failure.get().unwrap());
}
/// Physical-key capture currently has a verified X11 evdev+8 mapping. Other
/// native backends are rejected until their key namespace is explicitly mapped.
pub fn build(
    application: &gtk4::Application,
    local: LocalDevicePort,
    budget: MemoryBudget,
    observe: bool,
) -> Result<SourceWindow, Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1")
        || std::env::var("GDK_BACKEND").as_deref() != Ok("x11")
        || std::env::var_os("DISPLAY").is_none()
        || std::env::var_os("XAUTHORITY").is_none()
    {
        return Err("private authenticated X11 source display required".into());
    }
    // Trusted GTK/GDK opaque allowance is a bounded-profile charge, not an
    // instrumented claim about native allocator or GPU RSS.
    let native = Rc::new(budget.reserve(4 << 20)?);
    let renderer = Renderer::new_with_profile(
        RenderLimits::default(),
        budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )?;
    let canvas: Canvas = glib::Object::builder().build();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_focusable(true);
    canvas.set_cursor_from_name(Some("none"));
    let label = gtk4::Label::new(Some(
        "Isolated native source: physical keys use the host layout.",
    ));
    label.set_wrap(true);
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    content.append(&canvas);
    content.append(&label);
    let window = gtk4::ApplicationWindow::builder()
        .application(application)
        .title("RUI isolated source probe")
        .default_width(600)
        .default_height(400)
        .child(&content)
        .build();
    // Native object clones can outlive SourceWindow. Their finalization,
    // rather than the wrapper's drop, releases this shared opaque allowance.
    {
        let memory = native.clone();
        window.connect_destroy(move |_| {
            let _keep = &memory;
        });
    }
    {
        let memory = native.clone();
        canvas.connect_destroy(move |_| {
            let _keep = &memory;
        });
    }
    let painted = Rc::new(Cell::new(None));
    let failure = Rc::new(Cell::new(None));
    let counters = Rc::new(Counters::default());
    let active = Rc::new(Cell::new(false));
    let mut controllers = Vec::with_capacity(4);
    let focus = gtk4::EventControllerFocus::new();
    {
        let active = active.clone();
        focus.connect_enter(move |_| active.set(true));
    }
    {
        let active = active.clone();
        let local = local.clone();
        let failure = failure.clone();
        let label = label.clone();
        focus.connect_leave(move |_| {
            active.set(false);
            if local.cancel_input().is_err() {
                reject(
                    &failure,
                    &label,
                    "Local focus cleanup failed; restart the isolated source.",
                );
            }
        });
    }
    controllers.push(focus.clone().upcast());
    canvas.add_controller(focus);
    {
        let active = active.clone();
        let local = local.clone();
        canvas.connect_unmap(move |_| {
            active.set(false);
            let _ = local.cancel_input();
        });
    }
    {
        let active = active.clone();
        let local = local.clone();
        window.connect_close_request(move |_| {
            active.set(false);
            let _ = local.cancel_input();
            glib::Propagation::Proceed
        });
    }
    let keys = gtk4::EventControllerKey::new();
    {
        let local = local.clone();
        let failure = failure.clone();
        let label = label.clone();
        let counters = counters.clone();
        let active = active.clone();
        keys.connect_key_pressed(move |_, _, hardware, _| {
            if !active.get() {
                return glib::Propagation::Stop;
            }
            increment(&counters.pressed);
            let result = hardware
                .checked_sub(8)
                .ok_or(ShellError::Limit)
                .and_then(|code| {
                    local.native_event(LocalEvent::Key {
                        code,
                        pressed: true,
                    })
                });
            if result.is_err() {
                reject(
                    &failure,
                    &label,
                    "Local key was rejected; the source input queue or namespace is unavailable.",
                );
            }
            glib::Propagation::Stop
        });
    }
    {
        let local = local.clone();
        let failure = failure.clone();
        let label = label.clone();
        let counters = counters.clone();
        keys.connect_key_released(move |_, _, hardware, _| {
            increment(&counters.released);
            let result = hardware
                .checked_sub(8)
                .ok_or(ShellError::Limit)
                .and_then(|code| {
                    local.native_event(LocalEvent::Key {
                        code,
                        pressed: false,
                    })
                });
            if result.is_err() {
                reject(
                    &failure,
                    &label,
                    "Local key release was rejected; restart the isolated source to clear input.",
                );
            }
        });
    }
    controllers.push(keys.clone().upcast());
    canvas.add_controller(keys);
    let click = gtk4::GestureClick::new();
    click.set_button(1);
    {
        let weak = canvas.downgrade();
        let local = local.clone();
        let failure = failure.clone();
        let label = label.clone();
        let counters = counters.clone();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(canvas) = weak.upgrade() {
                canvas.grab_focus();
                let result = canvas.desktop_point(x, y).and_then(|desktop| {
                    local.native_event(LocalEvent::Pointer {
                        desktop,
                        press: true,
                    })
                });
                if result.is_ok() {
                    increment(&counters.pointer);
                } else {
                    reject(
                        &failure,
                        &label,
                        "Local pointer is outside the source canvas or could not be queued.",
                    );
                }
            }
        });
    }
    controllers.push(click.clone().upcast());
    canvas.add_controller(click);
    let motion = gtk4::EventControllerMotion::new();
    {
        let weak = canvas.downgrade();
        let local = local.clone();
        let failure = failure.clone();
        let label = label.clone();
        let counters = counters.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some(canvas) = weak.upgrade() {
                if let Ok(desktop) = canvas.desktop_point(x, y) {
                    if local
                        .native_event(LocalEvent::Pointer {
                            desktop,
                            press: false,
                        })
                        .is_ok()
                    {
                        increment(&counters.pointer);
                    } else {
                        reject(
                            &failure,
                            &label,
                            "Local pointer input queue is full or disconnected.",
                        );
                    }
                }
            }
        });
    }
    controllers.push(motion.clone().upcast());
    canvas.add_controller(motion);
    let clock_hook = Rc::new(RefCell::new(None));
    {
        let weak = canvas.downgrade();
        let painted = painted.clone();
        let clock_hook = clock_hook.clone();
        window.connect_realize(move |window| {
            if observe {
                if let Some(clock) = window.frame_clock() {
                    let weak = weak.clone();
                    let painted = painted.clone();
                    let handler = clock.connect_after_paint(move |_| {
                        if let Some(canvas) = weak.upgrade() {
                            if let Some(stamp) = canvas.imp().submitted.get() {
                                painted.set(Some(stamp));
                            }
                        }
                    });
                    clock_hook.replace(Some((clock, handler)));
                }
            }
        });
    }
    let weak_window = window.downgrade();
    let weak_canvas = canvas.downgrade();
    let pending = Rc::new(RefCell::new(None::<Pending>));
    let timer_failure = failure.clone();
    let local_timer = local.clone();
    let timer = glib::timeout_add_local(Duration::from_millis(16), move || {
        let failure = &timer_failure;
        if matches!(
            local_timer.source_health(),
            crate::actor::OwnerStatus::Poisoned | crate::actor::OwnerStatus::Stopped
        ) {
            reject(
                failure,
                &label,
                "Native owner is unavailable; restart the isolated source.",
            );
        }
        let (Some(window), Some(canvas)) = (weak_window.upgrade(), weak_canvas.upgrade()) else {
            return glib::ControlFlow::Break;
        };
        if !window.is_visible() {
            return glib::ControlFlow::Continue;
        }
        let result = pending.borrow().as_ref().map(Pending::try_recv);
        match result {
            Some(Ok(Some(reply))) => {
                pending.borrow_mut().take();
                let render = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let received = reply?;
                    let Response::Source(source) = received.value() else {
                        return Err("wrong source reply".into());
                    };
                    if canvas
                        .imp()
                        .stamp
                        .get()
                        .is_some_and(|stamp| stamp.revision == source.state().revision)
                    {
                        return Ok(());
                    }
                    let bounds = source
                        .state()
                        .records
                        .values()
                        .find_map(|record| match record {
                            Record::Session(session) => Some(session.canvas),
                            _ => None,
                        })
                        .ok_or("missing source canvas")?;
                    if bounds.width.fract() != 0.0
                        || bounds.height.fract() != 0.0
                        || bounds.width > 1024.0
                        || bounds.height > 1024.0
                    {
                        return Err("source window viewport limit".into());
                    }
                    let mut viewport = Viewport::new(bounds.width as u32, bounds.height as u32);
                    viewport.origin = Point {
                        x: bounds.x,
                        y: bounds.y,
                    };
                    let rgba = renderer
                        .render_view(source.state(), viewport, &())?
                        .into_rgba8_over(Color {
                            red: 1.0,
                            green: 1.0,
                            blue: 1.0,
                            alpha: 1.0,
                        })?;
                    let rgba8_sha256 = if observe {
                        Sha256::digest(rgba.as_ref()).into()
                    } else {
                        [0; 32]
                    };
                    let source_sha256 = if observe {
                        let _temporary = budget.reserve(1 << 20)?;
                        let bytes = pb::Snapshot {
                            revision: source.state().revision,
                            records: source.records().to_vec(),
                            ..Default::default()
                        }
                        .encode_to_vec();
                        Sha256::digest(bytes).into()
                    } else {
                        [0; 32]
                    };
                    let stamp = Painted {
                        revision: source.state().revision,
                        source_sha256,
                        rgba8_sha256,
                        canvas: bounds,
                        source_width: viewport.width,
                        source_height: viewport.height,
                    };
                    let bytes = glib::Bytes::from_owned(rgba);
                    let texture = gdk::MemoryTexture::new(
                        viewport.width as i32,
                        viewport.height as i32,
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        viewport.width as usize * 4,
                    );
                    canvas.imp().texture.replace(Some(texture));
                    canvas.imp().stamp.set(Some(stamp));
                    canvas.queue_draw();
                    if failure.get().is_none() {
                        let value = source
                            .state()
                            .records
                            .values()
                            .find_map(|record| match record {
                                Record::Semantic(node) => match &node.value {
                                    Some(TypedValue::Text(value)) => Some(value.as_str()),
                                    _ => None,
                                },
                                _ => None,
                            })
                            .unwrap_or("");
                        label.set_text(&format!("Native host value: {value}"));
                    }
                    Ok(())
                })();
                if render.is_err() {
                    reject(
                        failure,
                        &label,
                        "Source capture or rendering failed; restart the isolated source.",
                    );
                }
            }
            Some(Err(_)) => {
                pending.borrow_mut().take();
                reject(failure, &label, "Native owner stopped.");
            }
            _ => {}
        }
        if pending.borrow().is_none() {
            match local_timer.source_capture() {
                Ok(request) => {
                    pending.replace(Some(request));
                }
                Err(_) => reject(
                    failure,
                    &label,
                    "Source capture queue is full or disconnected.",
                ),
            }
        }
        glib::ControlFlow::Continue
    });
    Ok(SourceWindow {
        window,
        canvas,
        painted,
        counters,
        failure,
        timer: Some(timer),
        local,
        controllers,
        clock_hook,
        _native: native,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_mapping_rejects_letterbox_and_preserves_nonzero_origin() {
        let bounds = Rect {
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 120.0,
        };
        let (transform, clip) = mapping(800, 400, bounds).unwrap();
        let point = inverse_pointer(Point { x: 400.0, y: 200.0 }, transform, clip, bounds).unwrap();
        assert!((point.x - 110.0).abs() < 1e-10 && (point.y - 80.0).abs() < 1e-10);
        assert!(inverse_pointer(Point { x: 10.0, y: 200.0 }, transform, clip, bounds).is_err());
    }
}
