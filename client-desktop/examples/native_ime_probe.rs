//! Native key/preedit and retired-context probe on the harness's private Xvfb.
use gtk4::{gdk, gio, glib, prelude::*};
use rui_client_desktop::{
    desktop::{
        gtk::{queue_text_commit, LocalFeedback},
        native::NativeEditing,
        Stamp, UiCommand,
    },
    interaction::EditingContext,
    Viewport,
};
use rui_protocol_core::{model::TextRange, resources::MemoryBudget};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

fn main() {
    let app = gtk4::Application::builder()
        .application_id("org.rui.ImeProbe")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let passed = Rc::new(Cell::new(false));
    let final_result = passed.clone();
    app.connect_activate(move |app| {
        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .title("RUI native IME probe")
            .default_width(320)
            .default_height(120)
            .build();
        let column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let widget = gtk4::DrawingArea::new();
        widget.set_focusable(true);
        let preedit = gtk4::Label::new(None);
        let rejection = gtk4::Label::new(None);
        column.append(&preedit);
        column.append(&rejection);
        column.append(&widget);
        window.set_child(Some(&column));
        let context = EditingContext {
            epoch: [1; 16],
            view: [2; 16],
            lease: [3; 16],
            generation: 1,
            revision: 1,
            target: 20,
            target_version: 1,
            text: 40,
            text_record_version: 1,
            text_version: 1,
            focus_generation: 1,
            selection: TextRange {
                anchor: 0,
                focus: 0,
            },
        };
        let budget = MemoryBudget::new(1 << 20);
        let native_slot = Rc::new(RefCell::new(None::<NativeEditing>));
        let commits = Rc::new(RefCell::new(Vec::<String>::new()));
        let preedits = Rc::new(Cell::new(0));
        let old_instance = Rc::new(RefCell::new(None::<gtk4::IMMulticontext>));
        let native = {
            let slot = native_slot.clone();
            let commits = commits.clone();
            let app = app.clone();
            let passed = passed.clone();
            let preedit = preedit.clone();
            let preedits = preedits.clone();
            let old = old_instance.clone();
            let rejected_widget = widget.clone();
            let window = window.clone();
            let rejection = rejection.clone();
            let budget = budget.clone();
            NativeEditing::new(
                &widget,
                &preedit.clone(),
                Rc::new(RefCell::new(Some(budget.clone()))),
                move |mut captured, text| {
                    commits.borrow_mut().push(text.into());
                    let native = slot.borrow().as_ref().unwrap().clone();
                    if commits.borrow().len() == 1 {
                        assert_eq!(text, "a");
                        old.replace(native.ime());
                        captured.target = 99;
                        captured.focus_generation += 1;
                        captured.text_version += 1;
                        captured.text_record_version += 1;
                        native.reset(Some(captured), true);
                        let ime = native.ime().unwrap();
                        let old = old.clone();
                        let commits = commits.clone();
                        let preedits = preedits.clone();
                        let preedit = preedit.clone();
                        ime.connect_preedit_changed(move |ime| {
                            if !ime.preedit_string().0.is_empty() {
                                preedits.set(preedits.get() + 1);
                                let current = preedit.text();
                                // The new context's physical key already armed a capture.
                                // Retired callbacks must still never consume that capture.
                                old.borrow()
                                    .as_ref()
                                    .unwrap()
                                    .emit_by_name::<()>("commit", &[&"OLD-TARGET-LATE"]);
                                old.borrow()
                                    .as_ref()
                                    .unwrap()
                                    .emit_by_name::<()>("preedit-changed", &[]);
                                assert_eq!(commits.borrow().len(), 1);
                                assert_eq!(preedit.text(), current);
                            }
                        });
                    } else {
                        assert_eq!(&*commits.borrow(), &["a", "é"]);
                        assert_eq!(captured.target, 99);
                        assert!(preedits.get() > 0);
                        assert!(preedit.text().is_empty());
                        let retired = native.ime().unwrap();
                        native.reset(None, false);
                        retired.emit_by_name::<()>("commit", &[&"RESET-LATE"]);
                        captured.selection = TextRange {
                            anchor: 0,
                            focus: 3,
                        };
                        native.reset(Some(captured), true);
                        let keys = gtk4::EventControllerKey::new();
                        native.route_key(&keys, gdk::Key::a, gdk::ModifierType::empty(), |_| {});
                        native.ime().unwrap().emit_by_name::<()>("commit", &[&""]);
                        assert_eq!(
                            commits.borrow().len(),
                            2,
                            "empty commit must not delete a selection"
                        );
                        retired.emit_by_name::<()>("commit", &[&"REARMED-LATE"]);
                        native
                            .ime()
                            .unwrap()
                            .emit_by_name::<()>("commit", &[&"UNARMED-LATE"]);
                        assert_eq!(commits.borrow().len(), 2);
                        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
                        sender
                            .try_send(UiCommand::Acquire)
                            .unwrap_or_else(|_| panic!());
                        let feedback = LocalFeedback::new(&rejection);
                        let failures = Rc::new(Cell::new(0));
                        let failed_native = {
                            let failures = failures.clone();
                            let budget = budget.clone();
                            let feedback = feedback.clone();
                            NativeEditing::new(
                                &rejected_widget,
                                &preedit,
                                Rc::new(RefCell::new(Some(budget.clone()))),
                                move |context, text| {
                                    failures.set(failures.get() + 1);
                                    queue_text_commit(
                                        &sender,
                                        &feedback,
                                        Some(Stamp {
                                            generation: 0,
                                            serial: 1,
                                            revision: 1,
                                            viewport: Viewport::new(1, 1),
                                            pixel_digest: [0; 32],
                                            records_digest: None,
                                        }),
                                        context,
                                        text,
                                        Some(&budget),
                                    )
                                },
                            )
                        };
                        failed_native.reset(Some(captured), true);
                        failed_native.route_key(
                            &keys,
                            gdk::Key::a,
                            gdk::ModifierType::empty(),
                            |_| {},
                        );
                        failed_native
                            .ime()
                            .unwrap()
                            .emit_by_name::<()>("commit", &[&"rejected"]);
                        assert_eq!(failures.get(), 1);
                        assert!(feedback.message().is_some());
                        assert!(matches!(receiver.try_recv(), Ok(UiCommand::Acquire)));
                        assert!(
                            receiver.try_recv().is_err(),
                            "failed commit must enqueue no command"
                        );
                        // Ordinary chrome updates and an automatic remote context
                        // reset must not erase feedback or enable more native input.
                        rejection.set_text("Ordinary remote status.");
                        feedback.restore();
                        failed_native.reset(feedback.editing_context(Some(captured)), true);
                        let blocked = Cell::new(false);
                        failed_native.route_key(
                            &keys,
                            gdk::Key::a,
                            gdk::ModifierType::empty(),
                            |_| blocked.set(true),
                        );
                        failed_native
                            .ime()
                            .unwrap()
                            .emit_by_name::<()>("commit", &[&"still-rejected"]);
                        assert!(blocked.get());
                        assert_eq!(failures.get(), 1);
                        assert_eq!(rejection.text(), feedback.message().unwrap());
                        let passed = passed.clone();
                        let app = app.clone();
                        let feedback = feedback.clone();
                        let rejection = rejection.clone();
                        window.frame_clock().unwrap().connect_after_paint(move |_| {
                            if passed.get() {
                                return;
                            }
                            assert_eq!(rejection.text(), feedback.message().unwrap());
                            // Only explicit reselection clears the local guard.
                            feedback.clear();
                            assert_eq!(feedback.editing_context(Some(captured)), Some(captured));
                            passed.set(true);
                            app.quit();
                        });
                        window.queue_draw();
                    }
                    true
                },
            )
        };
        native_slot.replace(Some(native.clone()));
        native.reset(Some(context), true);
        let keys = gtk4::EventControllerKey::new();
        {
            let native = native.clone();
            keys.connect_key_pressed(move |controller, key, _, modifiers| {
                native.route_key(controller, key, modifiers, |_| {})
            });
        }
        {
            let native = native.clone();
            keys.connect_key_released(move |controller, _, _, _| native.filter_release(controller));
        }
        widget.add_controller(keys);
        window.present();
        widget.grab_focus();
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_secs(15), move || app.quit());
    });
    app.run_with_args::<&str>(&[]);
    assert!(final_result.get(), "native IME probe did not complete");
    println!("native IME: ordinary key + visible Unicode preedit + retired callbacks after rearm + empty commit rejection + full-channel sticky rejection after paint passed");
}
