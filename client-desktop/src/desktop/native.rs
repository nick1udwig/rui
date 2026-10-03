//! Native IM instances have immutable generation identities. Reset retires an
//! entire instance, so even delayed callbacks after a new key cannot retarget.
use super::gtk::{route_key, Shortcut};
use crate::{
    composition::{NativeStage, MAX_COMPOSITION_BYTES},
    interaction::{EditingContext, LocalComposition},
};
use gtk4::{gdk, glib, prelude::*};
use rui_protocol_core::input::ScalarRange;
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Commit = Box<dyn Fn(EditingContext, &str) -> bool>;
type WireComposition = Box<dyn Fn(EditingContext, [u8; 16], NativeStage<'_>) -> bool>;
struct Inner {
    generation: Cell<u64>,
    local: RefCell<LocalComposition>,
    ime: RefCell<Option<gtk4::IMMulticontext>>,
    widget: glib::WeakRef<gtk4::Widget>,
    preedit: gtk4::Label,
    memory: RefCell<Option<MemoryReservation>>,
    budget: Rc<RefCell<Option<MemoryBudget>>>,
    commit: Commit,
    wire: Option<WireComposition>,
    wire_failure: Option<Box<dyn Fn(Option<EditingContext>)>>,
    wire_mode: Cell<bool>,
    wire_active: RefCell<Option<([u8; 16], EditingContext)>>,
    identity_prefix: [u8; 8],
    identity_counter: Cell<u64>,
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
        Self::build(widget, preedit, budget, Box::new(commit), None, None)
    }
    pub fn new_with_composition(
        widget: &impl IsA<gtk4::Widget>,
        preedit: &gtk4::Label,
        budget: Rc<RefCell<Option<MemoryBudget>>>,
        commit: impl Fn(EditingContext, &str) -> bool + 'static,
        composition: impl Fn(EditingContext, [u8; 16], NativeStage<'_>) -> bool + 'static,
        failure: impl Fn(Option<EditingContext>) + 'static,
    ) -> Self {
        Self::build(
            widget,
            preedit,
            budget,
            Box::new(commit),
            Some(Box::new(composition)),
            Some(Box::new(failure)),
        )
    }
    fn build(
        widget: &impl IsA<gtk4::Widget>,
        preedit: &gtk4::Label,
        budget: Rc<RefCell<Option<MemoryBudget>>>,
        commit: Commit,
        wire: Option<WireComposition>,
        wire_failure: Option<Box<dyn Fn(Option<EditingContext>)>>,
    ) -> Self {
        // This random prefix belongs to one local IM owner; the counter never
        // resets when focus, source or connection changes.
        let uuid = glib::uuid_string_random();
        let mut identity_prefix = [0; 8];
        let mut digits = uuid.bytes().filter(|byte| *byte != b'-');
        for byte in &mut identity_prefix {
            let high = (digits.next().unwrap() as char).to_digit(16).unwrap();
            let low = (digits.next().unwrap() as char).to_digit(16).unwrap();
            *byte = ((high << 4) | low) as u8;
        }
        Self(Rc::new(Inner {
            generation: Cell::new(0),
            local: RefCell::new(LocalComposition::default()),
            ime: RefCell::new(None),
            widget: widget.as_ref().downgrade(),
            preedit: preedit.clone(),
            memory: RefCell::new(None),
            budget,
            commit,
            wire,
            wire_failure,
            wire_mode: Cell::new(false),
            wire_active: RefCell::new(None),
            identity_prefix,
            identity_counter: Cell::new(0),
        }))
    }
    /// The worker selects this only from the advertised CLIENT_TEXT mode and
    /// current TextComposition grant. Call reset after changing the mode.
    pub fn set_wire_mode(&self, enabled: bool) {
        self.0.wire_mode.set(enabled);
    }
    fn wire_begin(inner: &Inner, captured: EditingContext) -> bool {
        if let Some((_, owner)) = *inner.wire_active.borrow() {
            return owner == captured;
        }
        let Some(counter) = inner.identity_counter.get().checked_add(1) else {
            return false;
        };
        inner.identity_counter.set(counter);
        let mut id = [0; 16];
        id[..8].copy_from_slice(&inner.identity_prefix);
        id[8..].copy_from_slice(&counter.to_be_bytes());
        if inner
            .wire
            .as_ref()
            .is_some_and(|wire| wire(captured, id, NativeStage::Begin))
        {
            inner.wire_active.replace(Some((id, captured)));
            true
        } else {
            Self::wire_failed(inner, Some(captured));
            false
        }
    }
    fn wire_failed(inner: &Inner, owner: Option<EditingContext>) {
        if let Some(failure) = &inner.wire_failure {
            failure(owner.or_else(|| inner.local.borrow().current()));
        }
        inner.wire_active.replace(None);
        inner.local.borrow_mut().invalidate();
        inner
            .generation
            .set(inner.generation.get().saturating_add(1));
        Self::clear_preedit(inner);
    }
    fn wire_cancel(inner: &Inner) {
        let active = inner.wire_active.borrow_mut().take();
        if let Some((id, context)) = active {
            if !inner
                .wire
                .as_ref()
                .is_some_and(|wire| wire(context, id, NativeStage::Cancel))
            {
                Self::wire_failed(inner, Some(context));
            }
        }
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
        Self::wire_cancel(&self.0);
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
                Self::wire_cancel(&inner);
                inner.local.borrow_mut().allow_next_key();
                return;
            }
            let captured = inner.local.borrow_mut().take_commit();
            if let Some(captured) = captured {
                let accepted = if inner.wire_mode.get() {
                    if Self::wire_begin(&inner, captured) {
                        let (id, owner) = inner.wire_active.borrow_mut().take().unwrap();
                        inner
                            .wire
                            .as_ref()
                            .is_some_and(|wire| wire(owner, id, NativeStage::Commit(text)))
                    } else {
                        false
                    }
                } else {
                    (inner.commit)(captured, text)
                };
                if accepted && inner.generation.get() == generation {
                    inner.local.borrow_mut().allow_next_key();
                } else if inner.wire_mode.get() {
                    Self::wire_failed(&inner, Some(captured));
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
            let (text, _, cursor) = ime.preedit_string();
            if !inner.local.borrow().can_filter_keys() {
                Self::clear_preedit(&inner);
                return;
            }
            if inner.wire_mode.get() {
                let context = inner.local.borrow().current();
                if let Some(context) = context {
                    if (!text.is_empty() || inner.wire_active.borrow().is_some())
                        && Self::wire_begin(&inner, context)
                    {
                        let active = *inner.wire_active.borrow();
                        let Some((id, owner)) = active else {
                            return;
                        };
                        let caret = u64::try_from(cursor)
                            .ok()
                            .filter(|v| *v <= text.chars().count() as u64);
                        if !caret.is_some_and(|caret| {
                            inner.wire.as_ref().is_some_and(|wire| {
                                wire(
                                    owner,
                                    id,
                                    NativeStage::Update {
                                        text: &text,
                                        selection: ScalarRange {
                                            start: caret,
                                            end: caret,
                                        },
                                    },
                                )
                            })
                        }) {
                            Self::wire_failed(&inner, Some(owner));
                            return;
                        }
                    } else if !text.is_empty() {
                        return;
                    }
                }
            }
            if text.is_empty() {
                Self::clear_preedit(&inner);
                return;
            }
            // The local trusted GTK IM module owns its temporary GString. The
            // displayed copy is capped and reserved before GTK retains it.
            let memory = (text.len()
                <= if inner.wire_mode.get() {
                    MAX_COMPOSITION_BYTES
                } else {
                    1024
                })
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
                None => {
                    if inner.wire_mode.get() {
                        Self::wire_failed(&inner, None);
                    } else {
                        inner.local.borrow_mut().invalidate();
                        Self::clear_preedit(&inner);
                    }
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        ime.connect_preedit_start(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.generation.get() == generation
                && inner.wire_mode.get()
                && inner.local.borrow().can_filter_keys()
            {
                let context = inner.local.borrow().current();
                if let Some(context) = context {
                    Self::wire_begin(&inner, context);
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        ime.connect_preedit_end(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.generation.get() != generation || !inner.wire_mode.get() {
                return;
            }
            let active = *inner.wire_active.borrow();
            // GTK often emits empty/end immediately before commit. The same IM
            // generation may consume the active ID synchronously before idle.
            let weak = Rc::downgrade(&inner);
            glib::idle_add_local_once(move || {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                if inner.generation.get() == generation
                    && inner.wire_active.borrow().as_ref().map(|v| v.0) == active.map(|v| v.0)
                {
                    Self::wire_cancel(&inner);
                    inner.local.borrow_mut().allow_next_key();
                    Self::clear_preedit(&inner);
                }
            });
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
