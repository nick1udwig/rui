//! Native IM instances have immutable generation identities. Reset retires an
//! entire instance, so even delayed callbacks after a new key cannot retarget.
use super::gtk::{route_key, Shortcut};
use crate::interaction::{EditingContext, LocalComposition};
use gtk4::{gdk, glib, prelude::*};
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Commit = Box<dyn Fn(EditingContext, &str) -> bool>;
struct Inner {
    generation: Cell<u64>,
    local: RefCell<LocalComposition>,
    ime: RefCell<Option<gtk4::IMMulticontext>>,
    widget: glib::WeakRef<gtk4::Widget>,
    preedit: gtk4::Label,
    memory: RefCell<Option<MemoryReservation>>,
    budget: Rc<RefCell<Option<MemoryBudget>>>,
    commit: Commit,
}
#[derive(Clone)]
pub struct NativeEditing(Rc<Inner>);
impl NativeEditing {
    pub fn new(
        widget: &impl IsA<gtk4::Widget>,
        preedit: &gtk4::Label,
        budget: Rc<RefCell<Option<MemoryBudget>>>,
        commit: impl Fn(EditingContext, &str) -> bool + 'static,
    ) -> Self {
        Self(Rc::new(Inner {
            generation: Cell::new(0),
            local: RefCell::new(LocalComposition::default()),
            ime: RefCell::new(None),
            widget: widget.as_ref().downgrade(),
            preedit: preedit.clone(),
            memory: RefCell::new(None),
            budget,
            commit: Box::new(commit),
        }))
    }
    pub fn ime(&self) -> Option<gtk4::IMMulticontext> {
        self.0.ime.borrow().clone()
    }
    fn clear_preedit(inner: &Inner) {
        inner.preedit.set_text("");
        inner.memory.replace(None);
    }
    pub fn reset(&self, context: Option<EditingContext>, focused: bool) {
        self.0.local.borrow_mut().invalidate();
        Self::clear_preedit(&self.0);
        let Some(generation) = self.0.generation.get().checked_add(1) else {
            self.0.ime.replace(None);
            return;
        };
        self.0.generation.set(generation);
        let previous = self.0.ime.borrow_mut().take();
        if let Some(previous) = previous {
            previous.reset();
            previous.focus_out();
        }
        let Some(widget) = self.0.widget.upgrade() else {
            return;
        };
        let ime = gtk4::IMMulticontext::new();
        ime.set_client_widget(Some(&widget));
        let weak = Rc::downgrade(&self.0);
        ime.connect_commit(move |_, text| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.generation.get() != generation {
                return;
            }
            Self::clear_preedit(&inner);
            if text.is_empty() {
                inner.local.borrow_mut().allow_next_key();
                return;
            }
            let captured = inner.local.borrow_mut().take_commit();
            if let Some(captured) = captured {
                if (inner.commit)(captured, text) && inner.generation.get() == generation {
                    inner.local.borrow_mut().allow_next_key();
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        ime.connect_preedit_changed(move |ime| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.generation.get() != generation {
                return;
            }
            let (text, _, _) = ime.preedit_string();
            if text.is_empty() || !inner.local.borrow().can_filter_keys() {
                Self::clear_preedit(&inner);
                return;
            }
            // The local trusted GTK IM module owns its temporary GString. The
            // displayed copy is capped and reserved before GTK retains it.
            let memory = (text.len() <= 1024)
                .then(|| {
                    inner
                        .budget
                        .borrow()
                        .as_ref()?
                        .reserve(text.len() * 2 + 8192)
                        .ok()
                })
                .flatten();
            match memory {
                Some(memory) => {
                    inner.preedit.set_text(&text);
                    inner.memory.replace(Some(memory));
                }
                None => Self::clear_preedit(&inner),
            }
        });
        self.0.local.borrow_mut().install(context);
        self.0.ime.replace(Some(ime.clone()));
        if focused {
            ime.focus_in();
        }
    }
    pub fn route_key(
        &self,
        controller: &gtk4::EventControllerKey,
        key: gdk::Key,
        modifiers: gdk::ModifierType,
        mut shortcut: impl FnMut(Shortcut),
    ) -> glib::Propagation {
        let Some(ime) = self.ime() else {
            shortcut(Shortcut::BlockedTextInput);
            return glib::Propagation::Stop;
        };
        route_key(controller, key, modifiers, &ime, &self.0.local, shortcut)
    }
    pub fn filter_release(&self, controller: &gtk4::EventControllerKey) {
        if self.0.local.borrow().can_filter_keys() {
            if let (Some(ime), Some(event)) = (self.ime(), controller.current_event()) {
                ime.filter_keypress(&event);
            }
        }
    }
}
