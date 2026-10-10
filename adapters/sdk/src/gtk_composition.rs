//! One private, bounded composition owner behind the common command ledger.
use super::*;
use rui_protocol_core::commands::{CompositionOperation, TextCompositionCommand};
use rui_protocol_core::input::{CompositionMachine, EditingContext, InputError, TextOwner};

pub(super) const MAX_TEXT_BYTES: usize = 16 << 10;
pub(super) const COMPOSITION_MEMORY: usize = 256 << 10;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Context {
    pub target: u64,
    pub private_target: u64,
    pub surface: u64,
    pub target_version: u64,
    pub focus_generation: u64,
    pub native_focus_generation: u64,
    pub text_id: u64,
    pub text_record_version: u64,
    pub text_version: u64,
    pub native_text_version: u64,
    pub content: String,
    pub selection: Option<(u64, u64)>,
    pub source_generation: u64,
}
#[derive(Clone, PartialEq, Eq)]
struct Owner {
    client: AuthenticatedClient,
    lease: Id,
    generation: u64,
}
pub(super) struct Composition {
    machine: CompositionMachine,
    context: Option<Context>,
    owner: Option<Owner>,
    _memory: MemoryReservation,
}
impl Composition {
    pub fn new(budget: &MemoryBudget) -> Result<Self, GtkHostError> {
        Ok(Self {
            machine: CompositionMachine::new(128, MAX_TEXT_BYTES)
                .map_err(|_| GtkHostError::Unsupported)?,
            context: None,
            owner: None,
            _memory: budget.reserve(COMPOSITION_MEMORY)?,
        })
    }
    pub fn synchronize(&mut self, context: Option<Context>) -> bool {
        // Full text-record clocks include selection and editability. Comparing
        // only content_version would miss a selection away/back transition.
        let cancelled = self.context != context && self.machine.active_id().is_some();
        if self.context != context {
            self.cancel();
        }
        self.machine
            .set_context(context.as_ref().map(|value| EditingContext {
                target: value.target,
                focus_generation: value.focus_generation,
                text_version: value.text_version,
                scalar_count: value.content.chars().count() as u64,
                owner: TextOwner::ClientText,
            }));
        self.context = context;
        cancelled
    }
    pub fn cancel(&mut self) {
        self.machine.cancel();
        self.owner = None;
    }
    pub fn context(&self) -> Option<&Context> {
        self.context.as_ref()
    }
    pub fn validate(
        &self,
        client: &AuthenticatedClient,
        command: &TextCompositionCommand,
    ) -> Result<(), Rejection> {
        let context = self.context.as_ref().ok_or(Rejection::StaleText)?;
        if command.target_id != context.target
            || command.focus_generation != context.focus_generation
            || command.base_text_version != context.text_version
        {
            return Err(Rejection::StaleText);
        }
        if !matches!(command.operation, CompositionOperation::Begin(_))
            && self.owner.as_ref()
                != Some(&Owner {
                    client: client.clone(),
                    lease: command.header.control_lease_id,
                    generation: command.header.control_generation,
                })
        {
            return Err(Rejection::StaleText);
        }
        let result = match &command.operation {
            CompositionOperation::Begin(range) => self.machine.validate_begin(
                command.text_session_id,
                command.focus_generation,
                command.base_text_version,
                *range,
            ),
            CompositionOperation::UpdatePreedit { text, selection } => {
                if text.contains('\0') {
                    return Err(Rejection::InvalidArguments);
                }
                self.machine
                    .validate_update(command.text_session_id, text, *selection)
            }
            CompositionOperation::CommitText { text } => {
                if text.contains('\0') {
                    return Err(Rejection::InvalidArguments);
                }
                let range = self.machine.active_range().ok_or(Rejection::StaleText)?;
                let retained = context
                    .content
                    .chars()
                    .enumerate()
                    .filter(|(index, _)| {
                        (*index as u64) < range.start || (*index as u64) >= range.end
                    })
                    .map(|(_, c)| c.len_utf8())
                    .sum::<usize>();
                if retained
                    .checked_add(text.len())
                    .is_none_or(|bytes| bytes > MAX_TEXT_BYTES)
                {
                    return Err(Rejection::InvalidArguments);
                }
                self.machine.validate_commit(
                    command.text_session_id,
                    command.focus_generation,
                    command.base_text_version,
                    text,
                )
            }
            CompositionOperation::Cancel => {
                if self.machine.active_id() == Some(command.text_session_id) {
                    Ok(())
                } else {
                    Err(InputError::StaleComposition)
                }
            }
        };
        result.map_err(|error| match error {
            InputError::StaleComposition => Rejection::StaleText,
            InputError::TextOwnership => Rejection::Unauthorized,
            _ => Rejection::InvalidArguments,
        })
    }
    /// Called only after the native source acknowledged its transient state or
    /// entered commit dispatch. The common ledger is the only retry owner.
    pub fn acknowledge(
        &mut self,
        client: &AuthenticatedClient,
        command: &TextCompositionCommand,
    ) -> Result<(), DispatchFailure> {
        let result = match &command.operation {
            CompositionOperation::Begin(range) => self
                .machine
                .begin(
                    command.text_session_id,
                    command.focus_generation,
                    command.base_text_version,
                    *range,
                )
                .map(|_| {
                    self.owner = Some(Owner {
                        client: client.clone(),
                        lease: command.header.control_lease_id,
                        generation: command.header.control_generation,
                    });
                }),
            CompositionOperation::UpdatePreedit { text, selection } => {
                self.machine
                    .update(command.text_session_id, text, *selection)
            }
            CompositionOperation::CommitText { text } => self
                .machine
                .commit(
                    command.text_session_id,
                    command.focus_generation,
                    command.base_text_version,
                    text,
                )
                .map(|_| {
                    self.owner = None;
                }),
            CompositionOperation::Cancel => {
                self.cancel();
                Ok(())
            }
        };
        result.map_err(|_| DispatchFailure::OutcomeUnknown)
    }
}

pub(super) fn context(
    source: &ViewReceiver,
    identities: &SourceIdentities,
) -> Result<Option<Context>, GtkHostError> {
    let state = source.state().map_err(CaptureError::from)?;
    let Some(session) = state.records.values().find_map(|record| match record {
        Record::Session(value) => Some(value),
        _ => None,
    }) else {
        return Ok(None);
    };
    let Some(focus) = session.focus else {
        return Ok(None);
    };
    let Some(Record::Semantic(node)) = state.records.get(&focus.id) else {
        return Ok(None);
    };
    let Some(reference) = node.text else {
        return Ok(None);
    };
    let Some(Record::Text(text)) = state.records.get(&reference.id) else {
        return Ok(None);
    };
    let surface = semantic_surface(state, node.id)?;
    let allowed = authority(source, [1; 16])?.targets.contains_key(&node.id);
    if !allowed
        || session
            .focus_surface
            .is_none_or(|focus| focus.id != surface)
        || node.sensitivity != Sensitivity::Public
        || node.states.get(&StateKey::Disabled) == Some(&TriState::True)
        || !node.actions.iter().any(|action| action.id == "ReplaceText")
        || !text.editable
        || text.status != TextStatus::Available
    {
        return Ok(None);
    }
    let Some(content) = text
        .content
        .as_ref()
        .filter(|content| content.len() <= MAX_TEXT_BYTES)
    else {
        return Ok(None);
    };
    Ok(Some(Context {
        target: identities.public_id(node.id)?,
        private_target: node.id,
        surface,
        target_version: node.version,
        focus_generation: identities.focus_generation,
        native_focus_generation: session.focus_generation,
        text_id: text.id,
        text_record_version: text.version,
        text_version: text.text_version,
        native_text_version: text.text_version,
        content: content.clone(),
        selection: text
            .selection
            .map(|selection| (selection.anchor, selection.focus)),
        source_generation: identities.generation(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::{commands::CommandHeader, input::ScalarRange};
    fn client() -> AuthenticatedClient {
        AuthenticatedClient {
            principal: "approved".into(),
            client_instance_id: [2; 16],
        }
    }
    fn context() -> Context {
        Context {
            target: 9,
            private_target: 19,
            surface: 3,
            target_version: 4,
            focus_generation: 5,
            native_focus_generation: 5,
            text_id: 20,
            text_record_version: 6,
            text_version: 7,
            native_text_version: 7,
            content: "Aé😀e\u{301}Z".into(),
            selection: Some((1, 4)),
            source_generation: 1,
        }
    }
    fn command(operation: CompositionOperation) -> TextCompositionCommand {
        TextCompositionCommand {
            header: CommandHeader {
                epoch: [1; 16],
                view_id: [3; 16],
                client_instance_id: [2; 16],
                command_seq: 1,
                control_lease_id: [4; 16],
                control_generation: 8,
                observed_view_revision: 1,
            },
            target_id: 9,
            text_session_id: [5; 16],
            focus_generation: 5,
            base_text_version: 7,
            operation,
        }
    }
    fn owner() -> (Composition, MemoryBudget) {
        let budget = MemoryBudget::new(COMPOSITION_MEMORY);
        let mut composition = Composition::new(&budget).unwrap();
        composition.synchronize(Some(context()));
        (composition, budget)
    }
    fn begin(owner: &mut Composition) {
        let command = command(CompositionOperation::Begin(ScalarRange {
            start: 1,
            end: 4,
        }));
        owner.validate(&client(), &command).unwrap();
        owner.acknowledge(&client(), &command).unwrap();
    }
    #[test]
    fn scalar_edit_is_private_bound_and_one_shot_and_memory_is_released() {
        let (mut owner, budget) = owner();
        begin(&mut owner);
        let update = command(CompositionOperation::UpdatePreedit {
            text: "字😀e\u{301}".into(),
            selection: ScalarRange { start: 1, end: 4 },
        });
        owner.validate(&client(), &update).unwrap();
        owner.acknowledge(&client(), &update).unwrap();
        assert_eq!(owner.context().unwrap().content, "Aé😀e\u{301}Z");
        assert_eq!(owner.machine.preedit(), Some("字😀e\u{301}"));
        let commit = command(CompositionOperation::CommitText {
            text: "字😀".into(),
        });
        owner.validate(&client(), &commit).unwrap();
        owner.acknowledge(&client(), &commit).unwrap();
        assert_eq!(
            owner.validate(&client(), &commit),
            Err(Rejection::StaleText)
        );
        assert_eq!(
            owner.validate(
                &client(),
                &command(CompositionOperation::Begin(ScalarRange {
                    start: 1,
                    end: 4
                }))
            ),
            Err(Rejection::StaleText)
        );
        drop(owner);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn full_record_focus_source_and_native_target_aba_cancel_before_late_commit() {
        for change in 0..5 {
            let (mut owner, _) = owner();
            begin(&mut owner);
            let mut next = context();
            match change {
                0 => next.text_record_version += 2,
                1 => next.focus_generation += 2,
                2 => next.source_generation += 1,
                3 => next.private_target += 1,
                _ => next.selection = Some((0, 0)),
            }
            owner.synchronize(Some(next));
            assert!(owner.machine.preedit().is_none());
            assert_eq!(
                owner.validate(
                    &client(),
                    &command(CompositionOperation::CommitText {
                        text: "late".into()
                    })
                ),
                Err(Rejection::StaleText)
            );
        }
    }
    #[test]
    fn another_principal_or_lease_cannot_update_and_cancel_does_not_reuse_identity() {
        let (mut owner, _) = owner();
        begin(&mut owner);
        let mut update = command(CompositionOperation::UpdatePreedit {
            text: "private".into(),
            selection: ScalarRange { start: 0, end: 0 },
        });
        let mut other = client();
        other.principal = "other".into();
        assert_eq!(owner.validate(&other, &update), Err(Rejection::StaleText));
        update.header.control_generation += 1;
        assert_eq!(
            owner.validate(&client(), &update),
            Err(Rejection::StaleText)
        );
        owner.cancel();
        assert_eq!(
            owner.validate(
                &client(),
                &command(CompositionOperation::Begin(ScalarRange {
                    start: 0,
                    end: 0
                }))
            ),
            Err(Rejection::StaleText)
        );
    }
    #[test]
    fn original_range_and_resulting_utf8_bytes_are_bounded_before_native_mutation() {
        let (mut owner, _) = owner();
        let outside = command(CompositionOperation::Begin(ScalarRange {
            start: 0,
            end: 7,
        }));
        assert_eq!(
            owner.validate(&client(), &outside),
            Err(Rejection::InvalidArguments)
        );
        begin(&mut owner);
        let full = command(CompositionOperation::CommitText {
            text: "x".repeat(MAX_TEXT_BYTES),
        });
        assert_eq!(
            owner.validate(&client(), &full),
            Err(Rejection::InvalidArguments)
        );
        assert!(owner.machine.active_id().is_some());
    }

    #[test]
    fn retired_identity_retention_is_bounded_per_incarnation() {
        let (mut owner, _) = owner();
        for identity in 1..=128u8 {
            let mut begin = command(CompositionOperation::Begin(ScalarRange {
                start: 0,
                end: 0,
            }));
            begin.text_session_id = [identity; 16];
            owner.validate(&client(), &begin).unwrap();
            owner.acknowledge(&client(), &begin).unwrap();
            owner.cancel();
        }
        let mut extra = command(CompositionOperation::Begin(ScalarRange {
            start: 0,
            end: 0,
        }));
        extra.text_session_id = [129; 16];
        assert_eq!(
            owner.validate(&client(), &extra),
            Err(Rejection::InvalidArguments)
        );
        assert!(owner.machine.active_id().is_none());
    }
}
