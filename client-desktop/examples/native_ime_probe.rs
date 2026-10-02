//! Native key/preedit ordering probe, launched only on the harness's private Xvfb.
use gtk4::{gio, glib, prelude::*};
use rui_client_desktop::{
    desktop::gtk::{reset_composition, route_key},
    interaction::{EditingContext, LocalComposition},
};
use rui_protocol_core::model::TextRange;
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
        let widget = gtk4::DrawingArea::new();
        widget.set_focusable(true);
        window.set_child(Some(&widget));
        let local = Rc::new(RefCell::new(LocalComposition::default()));
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
        local.borrow_mut().install(Some(context));
        let ime = gtk4::IMMulticontext::new();
        ime.set_client_widget(Some(&widget));
        let commits = Rc::new(RefCell::new(Vec::<String>::new()));
        let preedits = Rc::new(Cell::new(0usize));
        {
            let preedits = preedits.clone();
            ime.connect_preedit_changed(move |ime| {
                if !ime.preedit_string().0.is_empty() {
                    preedits.set(preedits.get() + 1);
                }
            });
        }
        {
            let local = local.clone();
            let commits = commits.clone();
            let app = app.clone();
            let passed = passed.clone();
            let preedits = preedits.clone();
            ime.connect_commit(move |ime, text| {
                let Some(mut captured) = local.borrow_mut().take_commit() else {
                    return;
                };
                commits.borrow_mut().push(text.to_owned());
                captured.text_version += 1;
                captured.text_record_version += 1;
                local.borrow_mut().install(Some(captured));
                if commits.borrow().len() == 2 {
                    assert_eq!(&*commits.borrow(), &["a", "é"]);
                    assert!(preedits.get() > 0, "native preedit was not observed");
                    reset_composition(ime, &local, None);
                    ime.emit_by_name::<()>("commit", &[&"RESET-LATE"]);
                    local.borrow_mut().install(Some(captured));
                    ime.emit_by_name::<()>("commit", &[&"NEW-TARGET-LATE"]);
                    assert_eq!(commits.borrow().len(), 2);
                    passed.set(true);
                    app.quit();
                }
            });
        }
        // Same-target/no-op Focus completion uses the exact shell reset path.
        // It must rearm ordinary typing without admitting old preedit callbacks.
        local.borrow_mut().begin();
        reset_composition(&ime, &local, None);
        reset_composition(&ime, &local, Some(context));
        ime.emit_by_name::<()>("commit", &[&"OLD-CLICK-LATE"]);
        assert!(commits.borrow().is_empty());
        reset_composition(&ime, &local, Some(context));
        let keys = gtk4::EventControllerKey::new();
        {
            let ime = ime.clone();
            let local = local.clone();
            keys.connect_key_pressed(move |controller, key, _, modifiers| {
                route_key(controller, key, modifiers, &ime, &local, |_| {})
            });
        }
        {
            let ime = ime.clone();
            keys.connect_key_released(move |controller, _, _, _| {
                if let Some(event) = controller.current_event() {
                    ime.filter_keypress(&event);
                }
            });
        }
        widget.add_controller(keys);
        let focus = gtk4::EventControllerFocus::new();
        {
            let ime = ime.clone();
            focus.connect_enter(move |_| ime.focus_in());
        }
        widget.add_controller(focus);
        window.present();
        widget.grab_focus();
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_secs(15), move || app.quit());
    });
    app.run_with_args::<&str>(&[]);
    assert!(final_result.get(), "native IME probe did not complete");
    println!("native IME: ordinary key + Unicode preedit commit + reset/late rejection passed");
}
