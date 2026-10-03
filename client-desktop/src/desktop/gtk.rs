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
    fn display(&self, stamp: Stamp, pixels: RgbaFrame) {
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
        self.queue_draw();
    }
    fn clear(&self) {
        self.imp().texture.replace(None);
        self.imp().stamp.set(None);
        self.imp().snapshot_stamp.set(None);
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
            {
                let shutdown = shutdown_tx.clone();
                app.connect_shutdown(move |_| {
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
                runtime.block_on(worker(
                    options,
                    command_rx,
                    viewport_rx,
                    event_tx,
                    ui_tx,
                    shutdown_rx,
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
            for button in [&acquire, &release, &zoom_out, &zoom_in, &reset, &reconnect, &clear_cache] {
                toolbar.append(button);
            }
            toolbar.append(&label);
            column.append(&toolbar);
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
            let sender = command_tx.clone();
            let status = label.clone();
            acquire.connect_clicked(move |_| {
                enqueue(&sender, UiCommand::Acquire, &status);
            });
            {
                let sender = command_tx.clone();
                let label = label.clone();
                release.connect_clicked(move |_| {
                    enqueue(&sender, UiCommand::Release, &label);
                });
            }
            for (button,cache_loss) in [(reconnect,false),(clear_cache,true)] {
                let sender=command_tx.clone();let label=label.clone();
                button.connect_clicked(move |_| {
                    enqueue(&sender,if cache_loss {UiCommand::LoseResources}else{UiCommand::Reconnect},&label);
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
            click.set_button(1);
            {
                let canvas = canvas.downgrade();
                let sender = command_tx.clone();
                let label = label.clone();
                let native = native.clone();
                let state = ui_rx.clone();
                let observations = observations.clone();
                let click_pending = click_pending.clone();
                let feedback = feedback.clone();
                click.connect_pressed(move |_, _, x, y| {
                    feedback.clear();
                    click_pending.set(true);
                    if let Some(observations) = &observations {
                        observations.send_modify(|o| o.editing_target = None);
                    }
                    native.reset(None, true);
                    if let Some(canvas) = canvas.upgrade() {
                        canvas.grab_focus();
                        if let Some(stamp) = canvas.imp().stamp.get() {
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
            canvas.add_controller(click);
            let drag = gtk4::GestureDrag::new();
            drag.set_button(2);
            let drag_origin = Rc::new(Cell::new(Point { x: 0.0, y: 0.0 }));
            {
                let view = view.clone();
                let origin = drag_origin.clone();
                drag.connect_drag_begin(move |_, _, _| origin.set(view.get().origin));
            }
            {
                let view = view.clone();
                let origin = drag_origin;
                let tx = viewport_tx.clone();
                drag.connect_drag_update(move |_, x, y| {
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
                key.connect_key_pressed(move |controller, key, _, modifiers| {
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
                key.connect_key_released(move |controller, _, _, _| {
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
                    let editing = if click_pending.get() {
                        None
                    } else {
                        feedback.editing_context(state.borrow().editing)
                    };
                    native.reset(editing, true);
                    if let Some(observations) = &observations {
                        observations.send_modify(|o| {
                            o.editing_target = editing.map(|context| context.target)
                        });
                    }
                });
            }
            {
                let native = native.clone();
                let observations = observations.clone();
                focus.connect_leave(move |_| {
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
                window.connect_realize(move |window| {
                    if let Some(clock) = window.frame_clock() {
                        let canvas = weak_canvas.clone();
                        let sender = sender.clone();
                        let submitted = submitted.clone();
                        let label = label.clone();
                        let window = weak_window.clone();
                        let observations = observations.clone();
                        let ui = ui.clone();
                        clock.connect_after_paint(move |_| {
                            if let Some(observations) = &observations {
                                let current = *ui.borrow();
                                observations.send_modify(|o| {
                                    o.notice_visible = current
                                        .typing_notice
                                        .message()
                                        .is_some_and(|message| label.text() == message);
                                });
                            }
                            if let Some(canvas) = canvas.upgrade() {
                                if let Some(stamp) = canvas.imp().snapshot_stamp.get() {
                                    if submitted.get() != Some(stamp.serial) {
                                        if enqueue(&sender, UiCommand::Presented(stamp), &label) {
                                            submitted.set(Some(stamp.serial));
                                            if let (Some(observations), Some(window)) =
                                                (&observations, window.upgrade())
                                            {
                                                if let Some(origin) = canvas.compute_point(
                                                    &window,
                                                    &graphene::Point::new(0.0, 0.0),
                                                ) {
                                                    observations.send_modify(|o| {
                                                        o.displayed = Some(DisplayedObservation {
                                                            stamp,
                                                            canvas_origin: Point {
                                                                x: origin.x() as f64,
                                                                y: origin.y() as f64,
                                                            },
                                                            window_scale: window.scale_factor(),
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
                let old = previous.replace(current);
                if current.editing != old.editing
                    || current.editing_generation != old.editing_generation
                    || current.generation != old.generation
                    || current.lifecycle != old.lifecycle
                {
                    if current.editing_generation != old.editing_generation {
                        click_pending.set(false);
                    }
                    native.reset(
                        if canvas.has_focus() {
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
                    if current.generation != old.generation || current.lifecycle != Lifecycle::Live
                    {
                        canvas.clear();
                        submitted.set(None);
                        if let Some(observations) = &observations {
                            observations.send_modify(|o| {
                                o.displayed = None;
                                o.closed = matches!(
                                    current.lifecycle,
                                    Lifecycle::Closed | Lifecycle::Quit
                                );
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
                        Ok(UiEvent::Frame { stamp, pixels }) => {
                            if current.accepts(stamp) {
                                canvas.display(stamp, pixels);
                            }
                        }
                        Ok(UiEvent::Ready(memory)) => {
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
