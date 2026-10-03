//! Local physical mapping/repeat probe on the harness-owned X11 display.
//! This does not send remote input or claim broker/native-source integration.
use gtk4::{glib, prelude::*};
use rui_client_desktop::{
    desktop::physical_native,
    physical_input::{PhysicalKeyCapture, PhysicalKeyOwner},
};
use rui_protocol_core::resources::MemoryBudget;
use std::{cell::RefCell, rc::Rc};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1") {
        return Err("use native_smoke.py --probe physical on its private display".into());
    }
    let budget = MemoryBudget::new(64 * 1024);
    let capture = Rc::new(RefCell::new(PhysicalKeyCapture::new(&budget)?));
    let owner = PhysicalKeyOwner {
        connection_generation: 1,
        grant_serial: 1,
        surface: 1,
        routing_token: [1; 16],
    };
    let trace_memory = budget.reserve(16 * 1024)?;
    let trace = Rc::new(RefCell::new(Vec::new()));
    trace.borrow_mut().try_reserve_exact(16)?;
    let repeats = Rc::new(RefCell::new(0usize));
    let failed = Rc::new(RefCell::new(None::<&'static str>));
    let app = gtk4::Application::builder()
        .application_id("org.rui.PhysicalProbe")
        .build();
    {
        let budget = budget.clone();
        let capture = capture.clone();
        let trace = trace.clone();
        let repeats = repeats.clone();
        let failed = failed.clone();
        app.connect_activate(move |app| {
            let window = gtk4::ApplicationWindow::builder()
                .application(app)
                .title("RUI physical key probe")
                .default_width(320)
                .default_height(120)
                .build();
            let area = gtk4::DrawingArea::new();
            area.set_focusable(true);
            window.set_child(Some(&area));
            let controller = gtk4::EventControllerKey::new();
            {
                let app = app.clone();
                let capture = capture.clone();
                let trace = trace.clone();
                let repeats = repeats.clone();
                let failed = failed.clone();
                let budget = budget.clone();
                let display = gtk4::prelude::WidgetExt::display(&window);
                controller.connect_key_pressed(move |_, _, hardware, _| {
                    let event = physical_native::evdev_keycode(&display, hardware, &budget)
                        .ok_or("live XKB physical mapping unavailable")
                        .and_then(|code| {
                            capture
                                .borrow_mut()
                                .press(owner, hardware, code)
                                .map_err(|_| "physical key capture failed")
                        });
                    match event {
                        Ok(Some(key)) if trace.borrow().len() < 16 => trace.borrow_mut().push(key),
                        Ok(None) => *repeats.borrow_mut() += 1,
                        Err(error) => {
                            eprintln!("physical probe mapping failure: backend={}, hardware={hardware}, reason={error}, physical_position={:?}", display.type_().name(), physical_native::x11_key_name(&display, hardware, &budget));
                            failed.replace(Some("native physical press mapping/capture limit"));
                            app.quit();
                        }
                        Ok(Some(_)) => {
                            failed.replace(Some("native physical press mapping/capture limit"));
                            app.quit();
                        }
                    }
                    glib::Propagation::Stop
                });
            }
            {
                let app = app.clone();
                let capture = capture.clone();
                let trace = trace.clone();
                let failed = failed.clone();
                controller.connect_key_released(move |_, _, hardware, _| {
                    match capture.borrow_mut().release(owner, hardware) {
                        Ok(Some(key)) if trace.borrow().len() < 16 => trace.borrow_mut().push(key),
                        Ok(None) => {}
                        _ => {
                            failed.replace(Some("native physical release scope/capture limit"));
                            app.quit();
                        }
                    }
                    if trace.borrow().len() == 6 {
                        app.quit();
                    }
                });
            }
            area.add_controller(controller);
            window.present();
            area.grab_focus();
        });
    }
    app.run_with_args::<&str>(&[]);
    if let Some(error) = *failed.borrow() {
        return Err(error.into());
    }
    let actual: Vec<_> = trace
        .borrow()
        .iter()
        .map(|key| (key.code, key.pressed))
        .collect();
    assert_eq!(
        actual,
        [
            (21, true),
            (21, false),
            (42, true),
            (21, true),
            (21, false),
            (42, false)
        ]
    );
    assert!(
        *repeats.borrow() > 0,
        "actual native autorepeat must have been suppressed"
    );
    assert_eq!(capture.borrow().held_len(), 0);
    drop(trace_memory);
    println!("physical probe: actual X11 XKB positions, original press/release identity and native repeat suppression passed");
    Ok(())
}
