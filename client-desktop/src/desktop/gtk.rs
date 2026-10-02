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
            _ => None,
        };
        if let Some(local) = local {
            shortcut(local);
            return glib::Propagation::Stop;
        }
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
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::sync_channel(1);
        let (ui_tx, ui_rx) = watch::channel(UiState::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
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
            .title("RUI Mirror")
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
        let zoom_out = gtk4::Button::with_label("−");
        let zoom_in = gtk4::Button::with_label("+");
        let reset = gtk4::Button::with_label("Reset view");
        let label = gtk4::Label::new(Some("Connecting…"));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        for button in [&acquire, &zoom_out, &zoom_in, &reset] {
            toolbar.append(button);
        }
        toolbar.append(&label);
        column.append(&toolbar);
        let canvas = Canvas::new();
        let composition = Rc::new(RefCell::new(interaction::LocalComposition::default()));
        let ime = gtk4::IMMulticontext::new();
        ime.set_client_widget(Some(&canvas));
        column.append(&canvas);
        window.set_child(Some(&column));
        let sender = command_tx.clone();
        let status = label.clone();
        acquire.connect_clicked(move |_| {
            enqueue(&sender, UiCommand::Acquire, &status);
        });
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
            let composition = composition.clone();
            let ime = ime.clone();
            let state = ui_rx.clone();
            click.connect_pressed(move |_, _, x, y| {
                reset_composition(&ime, &composition, None);
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
                            reset_composition(&ime, &composition, state.borrow().editing);
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
        let budget = Rc::new(RefCell::new(None::<MemoryBudget>));
        {
            let budget = budget.clone();
            let canvas = canvas.downgrade();
            let sender = command_tx.clone();
            let label = label.clone();
            let composition = composition.clone();
            ime.connect_commit(move |_, text| {
                let Some(context) = composition.borrow_mut().take_commit() else {
                    return;
                };
                if text.len() > 1024 {
                    label.set_text("Text commit exceeds the local input limit.");
                    return;
                }
                if let (Some(canvas), Some(budget)) = (canvas.upgrade(), budget.borrow().as_ref()) {
                    if let Some(stamp) = canvas.imp().stamp.get() {
                        if let Ok(memory) = budget.reserve(text.len() + 2048) {
                            let mut owned = String::new();
                            if owned.try_reserve_exact(text.len()).is_ok() {
                                owned.push_str(text);
                                enqueue(
                                    &sender,
                                    UiCommand::Commit {
                                        stamp,
                                        context,
                                        text: owned,
                                        _memory: memory,
                                    },
                                    &label,
                                );
                            }
                        }
                    }
                }
            });
        }
        let key = gtk4::EventControllerKey::new();
        {
            let canvas = canvas.downgrade();
            let sender = command_tx.clone();
            let label = label.clone();
            let view = view.clone();
            let tx = viewport_tx.clone();
            let ime = ime.clone();
            let composition = composition.clone();
            key.connect_key_pressed(move |controller, key, _, modifiers| {
                route_key(
                    controller,
                    key,
                    modifiers,
                    &ime,
                    &composition,
                    |shortcut| match shortcut {
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
                    },
                )
            });
        }
        {
            let ime = ime.clone();
            key.connect_key_released(move |controller, _, _, _| {
                if let Some(event) = controller.current_event() {
                    ime.filter_keypress(&event);
                }
            });
        }
        canvas.add_controller(key);
        let focus = gtk4::EventControllerFocus::new();
        {
            let ime = ime.clone();
            let composition = composition.clone();
            let state = ui_rx.clone();
            focus.connect_enter(move |_| {
                composition.borrow_mut().install(state.borrow().editing);
                ime.focus_in();
            });
        }
        {
            let ime = ime.clone();
            let composition = composition.clone();
            focus.connect_leave(move |_| {
                composition.borrow_mut().invalidate();
                ime.reset();
                ime.focus_out();
            });
        }
        canvas.add_controller(focus);
        let submitted = Rc::new(Cell::new(None::<u64>));
        {
            let weak_canvas = canvas.downgrade();
            let sender = command_tx.clone();
            let submitted = submitted.clone();
            let label = label.clone();
            window.connect_realize(move |window| {
                if let Some(clock) = window.frame_clock() {
                    let canvas = weak_canvas.clone();
                    let sender = sender.clone();
                    let submitted = submitted.clone();
                    let label = label.clone();
                    clock.connect_after_paint(move |_| {
                        if let Some(canvas) = canvas.upgrade() {
                            if let Some(stamp) = canvas.imp().snapshot_stamp.get() {
                                if submitted.get() != Some(stamp.serial) {
                                    if enqueue(&sender, UiCommand::Presented(stamp), &label) {
                                        submitted.set(Some(stamp.serial));
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
            let current = *ui_rx.borrow();
            let old = previous.replace(current);
            if current != old {
                // Invalidate before reset: an IM module may synchronously commit.
                composition.borrow_mut().invalidate();
                ime.reset();
                if canvas.has_focus() {
                    composition.borrow_mut().install(current.editing);
                }
                if current.generation != old.generation || current.lifecycle != Lifecycle::Live {
                    canvas.clear();
                    submitted.set(None);
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
                        composition.borrow_mut().invalidate();
                        ime.reset();
                        label.set_text("Disconnected.");
                        return glib::ControlFlow::Break;
                    }
                }
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
    });
    app.run_with_args::<&str>(&[]);
    Ok(())
}
