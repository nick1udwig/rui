use super::*;
use gtk4::{gdk, gio, glib, graphene, prelude::*, subclass::prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
};

mod imp {
    use super::*;
    type Resize = Box<dyn Fn(i32, i32)>;
    #[derive(Default)]
    pub struct Canvas {
        pub texture: RefCell<Option<gdk::MemoryTexture>>,
        pub stamp: Cell<Option<Stamp>>,
        pub snapshot_stamp: Cell<Option<Stamp>>,
        pub actual_stamp: Cell<Option<Stamp>>,
        pub routing: RefCell<Option<std::sync::Arc<physical_dispatch::PresentedRouting>>>,
        pub snapshot_routing: RefCell<Option<std::sync::Arc<physical_dispatch::PresentedRouting>>>,
        pub actual_routing: RefCell<Option<std::sync::Arc<physical_dispatch::PresentedRouting>>>,
        pub resized: RefCell<Option<Resize>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "RuiMirrorCanvas";
        type Type = super::Canvas;
        type ParentType = gtk4::Widget;
    }
    impl ObjectImpl for Canvas {}
    impl WidgetImpl for Canvas {
        fn measure(&self, orientation: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (
                32,
                if orientation == gtk4::Orientation::Horizontal {
                    640
                } else {
                    480
                },
                -1,
                -1,
            )
        }
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(callback) = self.resized.borrow().as_ref() {
                callback(width, height);
            }
        }
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32),
            );
            if let (Some(texture), Some(stamp)) = (self.texture.borrow().as_ref(), self.stamp.get())
            {
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(
                        0.0,
                        0.0,
                        stamp.viewport.width as f32,
                        stamp.viewport.height as f32,
                    ),
                );
                self.snapshot_stamp.set(Some(stamp));
                self.snapshot_routing.replace(self.routing.borrow().clone());
            }
        }
    }
}
glib::wrapper! { pub struct Canvas(ObjectSubclass<imp::Canvas>) @extends gtk4::Widget, @implements gtk4::Accessible,gtk4::Buildable,gtk4::ConstraintTarget; }
impl Canvas {
    fn new() -> Self {
        let canvas: Self = glib::Object::builder().build();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        canvas.set_focusable(true);
        canvas
    }
    fn display(
        &self,
        stamp: Stamp,
        pixels: RgbaFrame,
        routing: Option<std::sync::Arc<physical_dispatch::PresentedRouting>>,
    ) {
        let bytes = glib::Bytes::from_owned(pixels);
        let texture = gdk::MemoryTexture::new(
            stamp.viewport.width as i32,
            stamp.viewport.height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            stamp.viewport.width as usize * 4,
        );
        self.imp().texture.replace(Some(texture));
        self.imp().stamp.set(Some(stamp));
        self.imp().routing.replace(routing);
        self.queue_draw();
    }
    fn clear(&self) {
        self.imp().texture.replace(None);
        self.imp().stamp.set(None);
        self.imp().snapshot_stamp.set(None);
        self.imp().actual_stamp.set(None);
        self.imp().routing.replace(None);
        self.imp().snapshot_routing.replace(None);
        self.imp().actual_routing.replace(None);
        self.queue_draw();
    }
}
#[derive(Clone)]
pub struct LocalFeedback {
    label: gtk4::Label,
    failure: Rc<Cell<Option<&'static str>>>,
}
impl LocalFeedback {
    pub fn new(label: &gtk4::Label) -> Self {
        Self {
            label: label.clone(),
            failure: Rc::new(Cell::new(None)),
        }
    }
    pub fn reject(&self, message: &'static str) {
        if self.failure.get().is_none() {
            self.failure.set(Some(message));
        }
        self.restore();
    }
    pub fn clear(&self) {
        self.failure.set(None);
    }
    pub fn message(&self) -> Option<&'static str> {
        self.failure.get()
    }
    pub fn restore(&self) {
        if let Some(message) = self.message() {
            self.label.set_text(message);
        }
    }
    pub fn editing_context(&self, context: Option<EditingContext>) -> Option<EditingContext> {
        if self.message().is_some() {
            None
        } else {
            context
        }
    }
}
fn enqueue(
    sender: &tokio::sync::mpsc::Sender<UiCommand>,
    command: UiCommand,
    status: &gtk4::Label,
) -> bool {
    if sender.try_send(command).is_err() {
        status.set_text("Input queue is full or disconnected.");
        false
    } else {
        true
    }
}
fn observe_action(
    observations: &Option<tokio::sync::watch::Sender<DesktopObservation>>,
    stamp: Stamp,
    kind: ActionKind,
    started_us: i64,
) {
    if let Some(observations) = observations {
        observations.send_modify(|o| {
            o.last_action = Some(ActionObservation {
                started_us,
                observed_revision: stamp.revision,
                kind,
            })
        });
    }
}

/// The shell and private native probe share this bounded commit handoff.
/// A failure disables native editing until explicit target reselection.
pub fn queue_text_commit(
    sender: &tokio::sync::mpsc::Sender<UiCommand>,
    feedback: &LocalFeedback,
    stamp: Option<Stamp>,
    context: EditingContext,
    text: &str,
    budget: Option<&MemoryBudget>,
) -> bool {
    if feedback.message().is_some() {
        feedback.restore();
        return false;
    }
    if text.len() > 1024 {
        feedback.reject("Text commit exceeds the local input limit. Select the target to resume.");
        return false;
    }
    if let (Some(stamp), Some(budget)) = (stamp, budget) {
        if let Ok(memory) = budget.reserve(text.len() + 2048) {
            let mut owned = String::new();
            if owned.try_reserve_exact(text.len()).is_ok() {
                owned.push_str(text);
                if sender
                    .try_send(UiCommand::Commit {
                        stamp,
                        context,
                        text: owned,
                        _memory: memory,
                    })
                    .is_ok()
                {
                    return true;
                }
            }
        }
    }
    feedback.reject(
        "Text input could not be queued within the local limit. Select the target to resume.",
    );
    false
}

/// Arms the original remote context before native IM filtering may emit a
/// synchronous commit. Exposed for the isolated native signal-order probe.
pub fn filter_text_key(
    controller: &gtk4::EventControllerKey,
    ime: &gtk4::IMMulticontext,
    composition: &RefCell<interaction::LocalComposition>,
) -> glib::Propagation {
    composition.borrow_mut().begin();
    if controller
        .current_event()
        .is_some_and(|event| ime.filter_keypress(&event))
    {
        glib::Propagation::Stop
    } else {
        glib::Propagation::Proceed
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    SelectAll,
    Pan { x: i32, y: i32 },
    BlockedTextInput,
    Zoom { direction: i32 },
    ReleaseControl,
    AcquireControl,
}
/// Complete mirror key route: only explicit local shortcuts bypass native IM.
pub fn route_key(
    controller: &gtk4::EventControllerKey,
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    ime: &gtk4::IMMulticontext,
    composition: &RefCell<interaction::LocalComposition>,
    mut shortcut: impl FnMut(Shortcut),
) -> glib::Propagation {
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        let local = match key {
            gdk::Key::a => Some(Shortcut::SelectAll),
            gdk::Key::Left => Some(Shortcut::Pan { x: -1, y: 0 }),
            gdk::Key::Right => Some(Shortcut::Pan { x: 1, y: 0 }),
            gdk::Key::Up => Some(Shortcut::Pan { x: 0, y: -1 }),
            gdk::Key::Down => Some(Shortcut::Pan { x: 0, y: 1 }),
            gdk::Key::plus | gdk::Key::equal => Some(Shortcut::Zoom { direction: 1 }),
            gdk::Key::minus => Some(Shortcut::Zoom { direction: -1 }),
            gdk::Key::Escape => Some(Shortcut::ReleaseControl),
            gdk::Key::F2 => Some(Shortcut::AcquireControl),
            _ => None,
        };
        if let Some(local) = local {
            shortcut(local);
            return glib::Propagation::Stop;
        }
    }
    if !composition.borrow().can_filter_keys() {
        shortcut(Shortcut::BlockedTextInput);
        return glib::Propagation::Stop;
    }
    filter_text_key(controller, ime, composition)
}
pub fn reset_composition(
    ime: &gtk4::IMMulticontext,
    composition: &RefCell<interaction::LocalComposition>,
    context: Option<EditingContext>,
) {
    composition.borrow_mut().invalidate();
    ime.reset();
    composition.borrow_mut().install(context);
}

/// Starts a local GTK shell. Credentials and application document data are never
/// printed; chrome controls act only on the connection or local viewport.
pub fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    run_many(vec![options])
}
/// Multiple independently authenticated mirrors in one GTK test process. Each
/// retains its own actor, receiver, ledger, viewport, IM instance and window.
pub fn run_many(options: Vec<Options>) -> Result<(), Box<dyn std::error::Error>> {
    if options.is_empty() || options.len() > 8 {
        return Err("mirror window count exceeds local limit".into());
    }
    let app = gtk4::Application::builder()
        .application_id("org.rui.Mirror")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let options = Rc::new(RefCell::new(Some(options)));
    let slot = options.clone();
    app.connect_activate(move |app| {
        let Some(options) = slot.borrow_mut().take() else {
            return;
        };
        for options in options {
            let title = options.title;
            let observations = options.observations.clone();
            let shutdown_signal = options.shutdown_signal.clone();
            let (command_tx, command_rx) = tokio::sync::mpsc::channel(32);
            let (event_tx, event_rx) = mpsc::sync_channel(1);
            let (ui_tx, ui_rx) = watch::channel(UiState::default());
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let (priority_tx, priority_rx) = watch::channel(None);
            {
                let shutdown = shutdown_tx.clone();
                let priority = priority_tx.clone();
                let current = ui_rx.clone();
                app.connect_shutdown(move |_| {
                    if let Some(control) = current.borrow().physical_control {
                        priority.send_replace(Some(control));
                    }
                    shutdown.send_replace(true);
                });
            }
            let view = Rc::new(Cell::new(Viewport::new(640, 480)));
            let (viewport_tx, viewport_rx) = watch::channel(view.get());
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("Tokio runtime");
                runtime.block_on(worker_with_priority(
                    options,
                    command_rx,
                    viewport_rx,
                    event_tx,
                    ui_tx,
                    shutdown_rx,
                    priority_rx,
                ));
            });
            let window = gtk4::ApplicationWindow::builder()
                .application(app)
                .title(title)
                .default_width(640)
                .default_height(540)
                .build();
            let column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            toolbar.set_margin_start(8);
            toolbar.set_margin_end(8);
            toolbar.set_margin_top(8);
            toolbar.set_margin_bottom(8);
            let acquire = gtk4::Button::with_label("Request control");
            let release = gtk4::Button::with_label("Release control");
            let zoom_out = gtk4::Button::with_label("−");
            let zoom_in = gtk4::Button::with_label("+");
            let reset = gtk4::Button::with_label("Reset view");
            let reconnect = gtk4::Button::with_label("Reconnect");
            let clear_cache = gtk4::Button::with_label("Clear cache");
            let label = gtk4::Label::new(Some("Connecting…"));
            label.set_hexpand(true);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            let feedback = LocalFeedback::new(&label);
            let canvas_feedback = LocalFeedback::new(&label);
            for button in [
                &acquire,
                &release,
                &zoom_out,
                &zoom_in,
                &reset,
                &reconnect,
                &clear_cache,
            ] {
                toolbar.append(button);
            }
            toolbar.append(&label);
            column.append(&toolbar);
            let source_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            source_bar.set_margin_start(8);
            source_bar.set_margin_end(8);
            let source_width = gtk4::SpinButton::with_range(1.0, 100_000.0, 10.0);
            let source_height = gtk4::SpinButton::with_range(1.0, 100_000.0, 10.0);
            source_width.set_width_chars(5);
            source_height.set_width_chars(5);
            let resize_source = gtk4::Button::with_label("Resize source");
            let scroll_y = gtk4::SpinButton::with_range(0.0, 1_000_000.0, 10.0);
            scroll_y.set_width_chars(5);
            let scroll_source = gtk4::Button::with_label("Scroll selected source");
            for widget in [
                &gtk4::Label::new(Some("Source size")).upcast::<gtk4::Widget>(),
                source_width.upcast_ref(),
                source_height.upcast_ref(),
                resize_source.upcast_ref(),
            ] {
                source_bar.append(widget);
            }
            resize_source.set_sensitive(false);
            scroll_source.set_sensitive(false);
            column.append(&source_bar);
            let scroll_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            scroll_bar.set_margin_start(8);
            scroll_bar.append(&gtk4::Label::new(Some("Absolute source Y")));
            scroll_bar.append(&scroll_y);
            scroll_bar.append(&scroll_source);
            column.append(&scroll_bar);
            let canvas = Canvas::new();
            let preedit = gtk4::Label::new(None);
            preedit.set_xalign(0.0);
            preedit.set_height_request(24);
            preedit.set_single_line_mode(true);
            preedit.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            preedit.set_max_width_chars(64);
            column.append(&preedit);
            column.append(&canvas);
            window.set_child(Some(&column));
            let budget = Rc::new(RefCell::new(None::<MemoryBudget>));
            let physical =
                super::physical_callbacks::PhysicalCallbacks::new(command_tx.clone(), priority_tx);
            let native = {
                let budget = budget.clone();
                let weak_canvas = canvas.downgrade();
                let sender = command_tx.clone();
                let feedback = feedback.clone();
                super::native::NativeEditing::new(
                    &canvas,
                    &preedit,
                    budget.clone(),
                    move |context, text| {
                        queue_text_commit(
                            &sender,
                            &feedback,
                            weak_canvas.upgrade().and_then(|c| c.imp().stamp.get()),
                            context,
                            text,
                            budget.borrow().as_ref(),
                        )
                    },
                )
            };
            native.reset(None, false);
            let click_pending = Rc::new(Cell::new(false));
            let source_stroke = Rc::new(RefCell::new(
                None::<(Stamp, Point, source_controls::PointerStroke)>,
            ));
            let stroke_clock = std::time::Instant::now();
            let loaded_source_size = Rc::new(Cell::new(None::<(u64, f64, f64)>));
            {
                let sender = command_tx.clone();
                let label = label.clone();
                let state = ui_rx.clone();
                let canvas = canvas.downgrade();
                let width = source_width.clone();
                let height = source_height.clone();
                let observations = observations.clone();
                resize_source.connect_clicked(move |_| {
                    let started_us = glib::monotonic_time();
                    if let (Some(target), Some(stamp)) = (
                        state.borrow().source_resize,
                        canvas.upgrade().and_then(|c| c.imp().stamp.get()),
                    ) {
                        observe_action(&observations, stamp, ActionKind::SourceResize, started_us);
                        enqueue(
                            &sender,
                            UiCommand::ResizeSource {
                                stamp,
                                target,
                                width: width.value(),
                                height: height.value(),
                            },
                            &label,
                        );
                    }
                });
            }
            {
                let sender = command_tx.clone();
                let label = label.clone();
                let state = ui_rx.clone();
                let canvas = canvas.downgrade();
                let y = scroll_y.clone();
                let observations = observations.clone();
                scroll_source.connect_clicked(move |_| {
                    let started_us = glib::monotonic_time();
                    if let (Some(target), Some(stamp)) = (
                        state.borrow().source_scroll,
                        canvas.upgrade().and_then(|c| c.imp().stamp.get()),
                    ) {
                        observe_action(&observations, stamp, ActionKind::SourceScroll, started_us);
                        enqueue(
                            &sender,
                            UiCommand::ScrollSource {
                                stamp,
                                target,
                                x: 0.0,
                                y: y.value(),
                            },
                            &label,
                        );
                    }
                });
            }
            let sender = command_tx.clone();
            let status = label.clone();
            acquire.connect_clicked(move |_| {
                enqueue(&sender, UiCommand::Acquire, &status);
            });
            {
                let sender = command_tx.clone();
                let label = label.clone();
                let physical = physical.clone();
                let state = ui_rx.clone();
                release.connect_clicked(move |_| {
                    physical.stop_current(*state.borrow());
                    enqueue(&sender, UiCommand::Release, &label);
                });
            }
            for (button, cache_loss) in [(&reconnect, false), (&clear_cache, true)] {
                let sender = command_tx.clone();
                let label = label.clone();
                button.connect_clicked(move |_| {
                    enqueue(
                        &sender,
                        if cache_loss {
                            UiCommand::LoseResources
                        } else {
                            UiCommand::Reconnect
                        },
                        &label,
                    );
                });
            }
            for (button, factor) in [(zoom_out, 0.8), (zoom_in, 1.25)] {
                let view = view.clone();
                let tx = viewport_tx.clone();
                button.connect_clicked(move |_| {
                    let mut v = view.get();
                    v.scale = (v.scale * factor).clamp(0.125, 8.0);
                    view.set(v);
                    tx.send_replace(v);
                });
            }
            {
                let view = view.clone();
                let tx = viewport_tx.clone();
                reset.connect_clicked(move |_| {
                    let mut v = view.get();
                    v.scale = 1.0;
                    v.origin = Point { x: 0.0, y: 0.0 };
                    view.set(v);
                    tx.send_replace(v);
                });
            }
            {
                let view = view.clone();
                let tx = viewport_tx.clone();
                canvas
                    .imp()
                    .resized
                    .replace(Some(Box::new(move |width, height| {
                        if width > 0 && height > 0 {
                            let mut v = view.get();
                            v.width = width as u32;
                            v.height = height as u32;
                            view.set(v);
                            tx.send_replace(v);
                        }
                    })));
            }
            let click = gtk4::GestureClick::new();
            click.set_button(0);
            {
                let canvas = canvas.downgrade();
                let sender = command_tx.clone();
                let label = label.clone();
                let native = native.clone();
                let state = ui_rx.clone();
                let observations = observations.clone();
                let click_pending = click_pending.clone();
                let feedback = feedback.clone();
                let canvas_feedback = canvas_feedback.clone();
                let physical = physical.clone();
                let budget = budget.clone();
                click.connect_pressed(move |gesture, _, x, y| {
                    let current = *state.borrow();
                    if current.physical_control.is_some() {
                        native.reset(None, false);
                        if let Some(canvas) = canvas.upgrade() {
                            canvas.grab_focus();
                            if let Err(message) = physical.pointer(
                                current,
                                canvas.imp().actual_stamp.get(),
                                canvas.imp().actual_routing.borrow().clone(),
                                rui_protocol_core::input::Event::Button {
                                    code: gesture.current_button(),
                                    pressed: true,
                                    position: Point { x, y },
                                },
                                budget.borrow().as_ref(),
                            ) {
                                label.set_text(message);
                            }
                        }
                        return;
                    }
                    if gesture.current_button() != 1 {
                        return;
                    }
                    let started_us = glib::monotonic_time();
                    feedback.clear();
                    canvas_feedback.clear();
                    click_pending.set(true);
                    if let Some(observations) = &observations {
                        observations.send_modify(|o| o.editing_target = None);
                    }
                    native.reset(None, true);
                    if let Some(canvas) = canvas.upgrade() {
                        canvas.grab_focus();
                        if let Some(stamp) = canvas.imp().stamp.get() {
                            observe_action(&observations, stamp, ActionKind::SemanticClick, started_us);
                            if !enqueue(
                                &sender,
                                UiCommand::Click {
                                    stamp,
                                    point: Point { x, y },
                                },
                                &label,
                            ) {
                                click_pending.set(false);
                                native.reset(state.borrow().editing, true);
                            }
                        }
                    }
                });
            }
            {
                let physical = physical.clone();
                let state = ui_rx.clone();
                let budget = budget.clone();
                let canvas = canvas.downgrade();
                let label = label.clone();
                click.connect_released(move |gesture, _, x, y| {
                    let current = *state.borrow();
                    if current.physical_control.is_some() {
                        if let Err(message) = physical.pointer(
                            current,
                            canvas.upgrade().and_then(|c| c.imp().actual_stamp.get()),
                            canvas
                                .upgrade()
                                .and_then(|c| c.imp().actual_routing.borrow().clone()),
                            rui_protocol_core::input::Event::Button {
                                code: gesture.current_button(),
                                pressed: false,
                                position: Point { x, y },
                            },
                            budget.borrow().as_ref(),
                        ) {
                            label.set_text(message);
                        }
                    }
                });
            }
            {
                let physical = physical.clone();
                let state = ui_rx.clone();
                click.connect_cancel(move |_, _| {
                    if state.borrow().physical_control.is_some() {
                        physical.stop_current(*state.borrow());
                    }
                });
            }
            let motion = gtk4::EventControllerMotion::new();
            {
                let physical = physical.clone();
                let state = ui_rx.clone();
                let budget = budget.clone();
                let canvas = canvas.downgrade();
                let label = label.clone();
                motion.connect_motion(move |_, x, y| {
                    let current = *state.borrow();
                    if current.physical_control.is_some() {
                        if let Err(message) = physical.pointer(
                            current,
                            canvas.upgrade().and_then(|c| c.imp().actual_stamp.get()),
                            canvas
                                .upgrade()
                                .and_then(|c| c.imp().actual_routing.borrow().clone()),
                            rui_protocol_core::input::Event::Motion(Point { x, y }),
                            budget.borrow().as_ref(),
                        ) {
                            label.set_text(message);
                        }
                    }
                });
            }
            {
                let physical = physical.clone();
                motion.connect_leave(move |_| physical.pointer_leave());
            }
            canvas.add_controller(motion);
            let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
            {
                let physical = physical.clone();
                let state = ui_rx.clone();
                let budget = budget.clone();
                let canvas = canvas.downgrade();
                let label = label.clone();
                scroll.connect_scroll(move |controller, x, y| {
                    let current = *state.borrow();
                    if current.physical_control.is_none() {
                        return glib::Propagation::Proceed;
                    }
                    let Some(canvas) = canvas.upgrade() else {
                        return glib::Propagation::Stop;
                    };
                    // Current GDK event positions are surface-local; convert
                    // through GTK into the exact displayed canvas coordinate.
                    let position = controller
                        .current_event()
                        .and_then(|event| event.position())
                        .and_then(|(x, y)| {
                            let native = canvas.native()?;
                            let widget = native.dynamic_cast::<gtk4::Widget>().ok()?;
                            widget.compute_point(&canvas, &graphene::Point::new(x as f32, y as f32))
                        });
                    if let Some(point) = position {
                        if let Err(message) = physical.pointer(
                            current,
                            canvas.imp().actual_stamp.get(),
                            canvas.imp().actual_routing.borrow().clone(),
                            rui_protocol_core::input::Event::Wheel {
                                x,
                                y,
                                position: Point {
                                    x: point.x() as f64,
                                    y: point.y() as f64,
                                },
                            },
                            budget.borrow().as_ref(),
                        ) {
                            label.set_text(message);
                        }
                    } else {
                        physical.stop_current(current);
                        label.set_text("Physical wheel position is unavailable; control stopped.");
                    }
                    glib::Propagation::Stop
                });
            }
            canvas.add_controller(scroll);
            let source_drag = gtk4::GestureDrag::new();
            source_drag.set_button(1);
            source_drag.group_with(&click);
            {
                let stroke = source_stroke.clone();
                let canvas = canvas.downgrade();
                let state = ui_rx.clone();
                let budget = budget.clone();
                let label = label.clone();
                let canvas_feedback = canvas_feedback.clone();
                source_drag.connect_drag_begin(move |_, x, y| {
                    stroke.borrow_mut().take();
                    if !state.borrow().raw_input {
                        return;
                    }
                    let Some(stamp) = canvas.upgrade().and_then(|c| c.imp().stamp.get()) else {
                        return;
                    };
                    let point = Point { x, y };
                    let time = u64::try_from(stroke_clock.elapsed().as_micros()).unwrap_or(u64::MAX);
                    match budget.borrow().as_ref().and_then(|budget| {
                        source_controls::PointerStroke::begin(
                            stamp.revision,
                            stamp.viewport,
                            source_controls::PointerSample {
                                timestamp_us: time,
                                point,
                            },
                            budget,
                        )
                        .ok()
                    }) {
                        Some(capture) => {
                            stroke.replace(Some((stamp, point, capture)));
                            label.set_text(
                                "Canvas stroke submits on release; every captured sample is retained.",
                            );
                        }
                        None => canvas_feedback.reject(
                            "Canvas capture memory or geometry limit reached; stroke cancelled.",
                        ),
                    }
                });
            }
            {
                let stroke = source_stroke.clone();
                let canvas_feedback = canvas_feedback.clone();
                source_drag.connect_drag_update(move |_, x, y| {
                            let mut capture = stroke.borrow_mut();
                            let Some((_, origin, points)) = capture.as_mut() else {
                                return;
                            };
                            let time = u64::try_from(stroke_clock.elapsed().as_micros()).unwrap_or(u64::MAX);
                            let sample = source_controls::PointerSample {
                                timestamp_us: time,
                                point: Point { x: origin.x + x, y: origin.y + y },
                            };
                            if points.push(sample).is_err() {
                                capture.take();
                                canvas_feedback.reject(
                                    "Canvas stroke exceeded its sample limit or geometry; nothing was submitted.",
                                );
                            }
                        });
            }
            {
                let stroke = source_stroke.clone();
                let label = label.clone();
                let sender = command_tx.clone();
                let canvas_feedback = canvas_feedback.clone();
                let observations = observations.clone();
                source_drag.connect_drag_end(move |_, x, y| {
                            let started_us = glib::monotonic_time();
                            let Some((stamp, origin, mut points)) = stroke.borrow_mut().take() else {
                                return;
                            };
                            if points.samples().len() == 1 {
                                return;
                            }
                            let time = u64::try_from(stroke_clock.elapsed().as_micros()).unwrap_or(u64::MAX);
                            let sample = source_controls::PointerSample {
                                timestamp_us: time,
                                point: Point { x: origin.x + x, y: origin.y + y },
                            };
                            if points.finish(sample).is_err() {
                                canvas_feedback.reject(
                                    "Canvas stroke exceeded its sample limit or geometry; nothing was submitted.",
                                );
                                return;
                            }
                            observe_action(&observations, stamp, ActionKind::StrokeRelease, started_us);
                            if !enqueue(&sender, UiCommand::Stroke { stamp, stroke: points }, &label) {
                                canvas_feedback.reject(
                                    "Canvas input queue is full or disconnected; nothing was submitted.",
                                );
                            }
                        });
            }
            {
                let stroke = source_stroke.clone();
                let canvas_feedback = canvas_feedback.clone();
                source_drag.connect_cancel(move |_, _| {
                    if stroke.borrow_mut().take().is_some() {
                        canvas_feedback.reject("Unsubmitted canvas stroke cancelled.");
                    }
                });
            }
            canvas.add_controller(click);
            canvas.add_controller(source_drag);
            let drag = gtk4::GestureDrag::new();
            drag.set_button(2);
            let drag_origin = Rc::new(Cell::new(Point { x: 0.0, y: 0.0 }));
            {
                let view = view.clone();
                let origin = drag_origin.clone();
                let state = ui_rx.clone();
                drag.connect_drag_begin(move |gesture, _, _| {
                    if state.borrow().physical_control.is_some() {
                        gesture.set_state(gtk4::EventSequenceState::Denied);
                    } else {
                        origin.set(view.get().origin);
                    }
                });
            }
            {
                let view = view.clone();
                let origin = drag_origin;
                let tx = viewport_tx.clone();
                let state = ui_rx.clone();
                drag.connect_drag_update(move |_, x, y| {
                    if state.borrow().physical_control.is_some() {
                        return;
                    }
                    let mut v = view.get();
                    let p = origin.get();
                    v.origin = Point {
                        x: p.x - x / v.scale,
                        y: p.y - y / v.scale,
                    };
                    view.set(v);
                    tx.send_replace(v);
                });
            }
            canvas.add_controller(drag);
            let key = gtk4::EventControllerKey::new();
            {
                let canvas = canvas.downgrade();
                let sender = command_tx.clone();
                let label = label.clone();
                let view = view.clone();
                let tx = viewport_tx.clone();
                let native = native.clone();
                let feedback = feedback.clone();
                let stroke = source_stroke.clone();
                let canvas_feedback = canvas_feedback.clone();
                let physical = physical.clone();
                let state = ui_rx.clone();
                let budget = budget.clone();
                key.connect_key_pressed(move |controller, key, hardware, modifiers| {
                                let current=*state.borrow();
                                if current.physical_control.is_some() {
                                    native.reset(None,false);
                                    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key==gdk::Key::Escape {
                                        physical.stop_current(current);label.set_text("Physical control stopped; request fresh control.");return glib::Propagation::Stop;
                                    }
                                    if let Some(canvas)=canvas.upgrade(){if let Err(message)=physical.key_press(&WidgetExt::display(&canvas),hardware,current,canvas.imp().actual_stamp.get(),canvas.imp().actual_routing.borrow().clone(),budget.borrow().as_ref()){label.set_text(message);}}
                                    return glib::Propagation::Stop;
                                }
                                if key==gdk::Key::Escape && stroke.borrow_mut().take().is_some() {
                                    canvas_feedback.reject("Unsubmitted canvas stroke cancelled.");
                                    return glib::Propagation::Stop;
                                }
                                if sender.capacity() == 0 {
                                    feedback.reject("Typing queue is full; this key was not accepted. Select the target to resume.");
                                    native.reset(None, true);
                                    return glib::Propagation::Stop;
                                }
                                native.route_key(controller, key, modifiers, |shortcut| match shortcut {
                                    Shortcut::SelectAll => {
                                        if let Some(canvas) = canvas.upgrade() {
                                            if let Some(stamp) = canvas.imp().stamp.get() {
                                                enqueue(&sender, UiCommand::SelectAll { stamp }, &label);
                                            }
                                        }
                                    }
                                    Shortcut::Pan { x, y } => {
                                        let mut v = view.get();
                                        let distance = 30.0 / v.scale;
                                        v.origin.x += x as f64 * distance;
                                        v.origin.y += y as f64 * distance;
                                        view.set(v);
                                        tx.send_replace(v);
                                    }
                                    Shortcut::BlockedTextInput => label.set_text(
                                        "Text input is unavailable; select an editable remote target.",
                                    ),
                                    Shortcut::Zoom { direction } => {
                                        let mut v = view.get();
                                        v.scale = (v.scale * if direction > 0 { 1.25 } else { 0.8 })
                                            .clamp(0.125, 8.0);
                                        view.set(v);
                                        tx.send_replace(v);
                                    }
                                    Shortcut::ReleaseControl => {
                                        enqueue(&sender, UiCommand::Release, &label);
                                    }
                                    Shortcut::AcquireControl => {
                                        enqueue(&sender, UiCommand::Acquire, &label);
                                    }
                                })
                            });
            }
            {
                let native = native.clone();
                let physical = physical.clone();
                let state = ui_rx.clone();
                let budget = budget.clone();
                let canvas = canvas.downgrade();
                let label = label.clone();
                key.connect_key_released(move |controller, _, hardware, _| {
                    let current = *state.borrow();
                    if current.physical_control.is_some() {
                        native.reset(None, false);
                        if let Err(message) = physical.key_release(
                            hardware,
                            current,
                            canvas.upgrade().and_then(|c| c.imp().actual_stamp.get()),
                            canvas
                                .upgrade()
                                .and_then(|c| c.imp().actual_routing.borrow().clone()),
                            budget.borrow().as_ref(),
                        ) {
                            label.set_text(message);
                        }
                        return;
                    }
                    native.filter_release(controller);
                });
            }
            canvas.add_controller(key);
            let focus = gtk4::EventControllerFocus::new();
            {
                let native = native.clone();
                let state = ui_rx.clone();
                let click_pending = click_pending.clone();
                let observations = observations.clone();
                let feedback = feedback.clone();
                focus.connect_enter(move |_| {
                    let editing = if click_pending.get() || state.borrow().physical_control.is_some() {
                        None
                    } else {
                        feedback.editing_context(state.borrow().editing)
                    };
                    native.reset(editing, true);
                    if let Some(observations) = &observations {
                        observations
                            .send_modify(|o| o.editing_target = editing.map(|context| context.target));
                    }
                });
            }
            {
                let native = native.clone();
                let observations = observations.clone();
                let physical = physical.clone();
                let state = ui_rx.clone();
                focus.connect_leave(move |_| {
                    physical.stop_current(*state.borrow());
                    native.reset(None, false);
                    if let Some(observations) = &observations {
                        observations.send_modify(|o| o.editing_target = None);
                    }
                });
            }
            canvas.add_controller(focus);
            let submitted = Rc::new(Cell::new(None::<u64>));
            {
                let weak_canvas = canvas.downgrade();
                let weak_window = window.downgrade();
                let observations = observations.clone();
                let sender = command_tx.clone();
                let submitted = submitted.clone();
                let label = label.clone();
                let ui = ui_rx.clone();
                let canvas_feedback = canvas_feedback.clone();
                let controls = [
                    resize_source.upcast_ref::<gtk4::Widget>().downgrade(),
                    source_width.upcast_ref::<gtk4::Widget>().downgrade(),
                    source_height.upcast_ref::<gtk4::Widget>().downgrade(),
                    scroll_source.upcast_ref::<gtk4::Widget>().downgrade(),
                    scroll_y.upcast_ref::<gtk4::Widget>().downgrade(),
                    reconnect.upcast_ref::<gtk4::Widget>().downgrade(),
                    clear_cache.upcast_ref::<gtk4::Widget>().downgrade(),
                ];
                window.connect_realize(move |window| {
                                if let Some(clock) = window.frame_clock() {
                                    let canvas = weak_canvas.clone();
                                    let sender = sender.clone();
                                    let submitted = submitted.clone();
                                    let label = label.clone();
                                    let window = weak_window.clone();
                                    let observations = observations.clone();
                                    let ui = ui.clone();
                                    let canvas_feedback=canvas_feedback.clone();
                                    let controls=controls.clone();
                                    clock.connect_after_paint(move |clock| {
                                        if let Some(observations) = &observations {
                                            let current = *ui.borrow();
                                            observations.send_modify(|o| {
                                                o.notice_visible = current
                                                    .typing_notice
                                                    .message()
                                                    .is_some_and(|message| label.text() == message);
                                                o.stroke_notice = canvas_feedback.message();
                                                o.stroke_notice_visible = canvas_feedback
                                                    .message()
                                                    .is_some_and(|message| label.text() == message);
                                            });
                                        }
                                        if let Some(canvas) = canvas.upgrade() {
                                            if let Some(stamp) = canvas.imp().snapshot_stamp.get() {
                                                canvas.imp().actual_stamp.set(Some(stamp));
                                            canvas.imp().actual_routing.replace(canvas.imp().snapshot_routing.borrow().clone());
                                                if submitted.get() != Some(stamp.serial) {
                                                    if enqueue(&sender, UiCommand::Presented(stamp), &label) {
                                                        submitted.set(Some(stamp.serial));
                                                        if let (Some(observations), Some(window)) = (&observations, window.upgrade()) {
                                                            if let Some(origin) =
                                                                canvas.compute_point(&window, &graphene::Point::new(0.0, 0.0))
                                                            {
                                                                observations.send_modify(|o| {
                                                                    o.displayed = Some(DisplayedObservation {
                                                                        stamp,
                                                                        canvas_origin: Point {
                                                                            x: origin.x() as f64,
                                                                            y: origin.y() as f64,
                                                                        },
                                                                        window_scale: window.scale_factor(),
                                                                        frame_time_us: clock.frame_time(),
                                                                        refresh_interval_us: clock.refresh_info(clock.frame_time()).0,
                                                                        predicted_presentation_us: clock
                                                                            .refresh_info(clock.frame_time())
                                                                            .1,
                                                                        after_paint_time_us: glib::monotonic_time(),
                                                                    })
                                                                });
                                                            }
                                                            let points = controls.each_ref().map(|weak| {
                                                                weak.upgrade()
                                                                    .and_then(|widget| {
                                                                        widget.compute_point(
                                                                            &window,
                                                                            &graphene::Point::new(
                                                                                widget.width() as f32 / 2.0,
                                                                                widget.height() as f32 / 2.0,
                                                                            ),
                                                                        )
                                                                    })
                                                                    .map(|p| Point {
                                                                        x: p.x() as f64,
                                                                        y: p.y() as f64,
                                                                    })
                                                            });
                                                            if let [
                                                                Some(resize), Some(width), Some(height),
                                                                Some(scroll), Some(scroll_y), Some(reconnect), Some(cache),
                                                            ] = points
                                                            {
                                                                observations.send_modify(|o| {
                                                                    o.local_controls = Some(LocalControls {
                                                                        resize,
                                                                        width,
                                                                        height,
                                                                        scroll,
                                                                        scroll_y,
                                                                        reconnect,
                                                                        cache,
                                                                    })
                                                                });
                                                            }
                                                        }
                                                    } else {
                                                        canvas.queue_draw();
                                                    }
                                                }
                                            }
                                        }
                                    });
                                }
                            });
            }
            let app_weak = app.downgrade();
            let canvas_weak = canvas.downgrade();
            let label_weak = label.downgrade();
            let previous = Rc::new(Cell::new(UiState::default()));
            let close_physical = physical.clone();
            let close_ui = ui_rx.clone();
            let source = glib::timeout_add_local(Duration::from_millis(16), move || {
                let (Some(app), Some(canvas), Some(label)) = (
                    app_weak.upgrade(),
                    canvas_weak.upgrade(),
                    label_weak.upgrade(),
                ) else {
                    return glib::ControlFlow::Break;
                };
                if shutdown_signal
                    .as_ref()
                    .is_some_and(|signal| *signal.borrow())
                {
                    app.quit();
                    return glib::ControlFlow::Break;
                }
                let current = *ui_rx.borrow();
                physical.reconcile(current);
                let old = previous.replace(current);
                resize_source.set_sensitive(current.source_resize.is_some());
                scroll_source.set_sensitive(current.source_scroll.is_some());
                if let Some(target) = current.source_resize {
                    let size = (target.surface, target.width, target.height);
                    if loaded_source_size.get() != Some(size) {
                        source_width.set_value(target.width);
                        source_height.set_value(target.height);
                        loaded_source_size.set(Some(size));
                    }
                }
                if (!current.raw_input
                    || current.generation != old.generation
                    || current.lifecycle != Lifecycle::Live)
                    && source_stroke.borrow_mut().take().is_some()
                {
                    canvas_feedback.reject(
                        "Source readiness changed; the unsubmitted canvas stroke was cancelled.",
                    );
                }
                if current.editing != old.editing
                    || current.editing_generation != old.editing_generation
                    || current.generation != old.generation
                    || current.lifecycle != old.lifecycle
                {
                    if current.editing_generation != old.editing_generation {
                        click_pending.set(false);
                    }
                    native.reset(
                        if canvas.has_focus() && current.physical_control.is_none() {
                            feedback.editing_context(current.editing)
                        } else {
                            None
                        },
                        canvas.has_focus(),
                    );
                    if let Some(observations) = &observations {
                        observations.send_modify(|o| {
                            o.editing_target = if canvas.has_focus() && feedback.message().is_none() {
                                current.editing.map(|context| context.target)
                            } else {
                                None
                            }
                        });
                    }
                    if current.generation != old.generation || current.lifecycle != Lifecycle::Live {
                        canvas.clear();
                        submitted.set(None);
                        if let Some(observations) = &observations {
                            observations.send_modify(|o| {
                                o.displayed = None;
                                o.closed =
                                    matches!(current.lifecycle, Lifecycle::Closed | Lifecycle::Quit);
                            });
                        }
                    }
                }
                if current.lifecycle == Lifecycle::Quit {
                    app.quit();
                    return glib::ControlFlow::Break;
                }
                for _ in 0..4 {
                    match event_rx.try_recv() {
                        Ok(UiEvent::Frame {
                            stamp,
                            pixels,
                            routing,
                        }) => {
                            if current.accepts(stamp) {
                                canvas.display(stamp, pixels, routing);
                            }
                        }
                        Ok(UiEvent::Ready(memory)) => {
                            physical.replace_budget();
                            budget.replace(Some(memory));
                        }
                        Ok(UiEvent::Status(text)) => label.set_text(text),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            canvas.clear();
                            native.reset(None, false);
                            label.set_text("Disconnected.");
                            return glib::ControlFlow::Break;
                        }
                    }
                }
                if let Some(message) = current.typing_notice.message() {
                    label.set_text(message);
                }
                feedback.restore();
                canvas_feedback.restore();
                if let Some(message) = current.physical_notice {
                    label.set_text(message);
                }
                if let Some(observations) = &observations {
                    observations.send_modify(|o| {
                        if o.typing_notice != current.typing_notice {
                            o.notice_visible = false;
                        }
                        o.typing_notice = current.typing_notice;
                    });
                }
                glib::ControlFlow::Continue
            });
            let source = Rc::new(RefCell::new(Some(source)));
            window.connect_close_request(move |_| {
                close_physical.stop_current(*close_ui.borrow());
                shutdown_tx.send_replace(true);
                if let Some(source) = source.borrow_mut().take() {
                    source.remove();
                }
                glib::Propagation::Proceed
            });
            window.present();
        }
    });
    app.run_with_args::<&str>(&[]);
    Ok(())
}
