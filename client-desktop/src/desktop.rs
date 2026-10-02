//! Optional GTK mirror shell. All document state remains in NetworkClient's
//! receiver; local viewport changes never generate application commands.
use crate::{
    interaction::{self, CommandBinding, EditingContext},
    *,
};
use prost::Message;
use rui_protocol_core::{
    commands::Arguments,
    model::*,
    resources::{MemoryBudget, MemoryReservation},
    session::{ClientHello, ReceiverLimits, SessionConfig},
    wire::proto as core,
};
use rui_transport::{pairing::Credential, proto as session, ClientEvent, NetworkClient};
use std::{
    net::SocketAddr,
    sync::mpsc::{SyncSender, TrySendError},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

pub mod gtk;

pub struct Options {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: Vec<u8>,
    pub credential: Credential,
    pub client_id: [u8; 16],
    pub auto_control: bool,
    pub smoke_frames: Option<u64>,
}
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub generation: u64,
    pub serial: u64,
    pub revision: u64,
    pub viewport: Viewport,
}
pub enum UiCommand {
    Acquire,
    Presented(Stamp),
    Click {
        stamp: Stamp,
        point: Point,
    },
    Commit {
        stamp: Stamp,
        context: EditingContext,
        text: String,
        _memory: MemoryReservation,
    },
    SelectAll {
        stamp: Stamp,
    },
    Quit,
}
pub enum UiEvent {
    Frame { stamp: Stamp, pixels: RgbaFrame },
    Status(&'static str),
    Ready(MemoryBudget),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Live,
    Suspended,
    Closed,
    Quit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiState {
    pub generation: u64,
    pub editing_generation: u64,
    pub lifecycle: Lifecycle,
    pub editing: Option<EditingContext>,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            generation: 0,
            editing_generation: 0,
            lifecycle: Lifecycle::Live,
            editing: None,
        }
    }
}
impl UiState {
    pub fn clear(&mut self, lifecycle: Lifecycle) {
        if matches!(self.lifecycle, Lifecycle::Closed | Lifecycle::Quit) {
            return;
        }
        match self.generation.checked_add(1) {
            Some(generation) => {
                self.generation = generation;
                self.lifecycle = lifecycle;
            }
            None => self.lifecycle = Lifecycle::Quit,
        }
        self.editing = None;
    }
    pub fn resume(&mut self) -> bool {
        if self.lifecycle == Lifecycle::Suspended {
            self.lifecycle = Lifecycle::Live;
        }
        self.lifecycle == Lifecycle::Live
    }
    pub fn accepts(&self, stamp: Stamp) -> bool {
        self.lifecycle == Lifecycle::Live && self.generation == stamp.generation
    }
}

struct Lease {
    wire: session::ControlGranted,
    _memory: MemoryReservation,
}
fn wire_command(c: rui_protocol_core::commands::SemanticCommand) -> core::SemanticCommand {
    let arguments = match c.arguments {
        Arguments::None => core::semantic_command::Arguments::NoArguments(core::EmptyArguments {}),
        Arguments::Checked(checked) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Checked(checked)),
            })
        }
        Arguments::Number(number) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Number(number)),
            })
        }
        Arguments::ValueText(text) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Text(text)),
            })
        }
        Arguments::ReplaceText { start, end, text } => {
            core::semantic_command::Arguments::ReplaceText(core::TextArguments { start, end, text })
        }
        Arguments::Selection { anchor, focus } => {
            core::semantic_command::Arguments::SetSelection(core::SelectionArguments {
                anchor,
                focus,
            })
        }
        Arguments::Scroll { x, y } => {
            core::semantic_command::Arguments::ScrollTo(core::ScrollArguments { x, y })
        }
    };
    core::SemanticCommand {
        epoch: c.epoch.to_vec(),
        view_id: c.view_id.to_vec(),
        client_instance_id: c.client_instance_id.to_vec(),
        command_seq: c.command_seq,
        control_lease_id: c.control_lease_id.to_vec(),
        control_generation: c.control_generation,
        target_id: c.target_id,
        action_id: c.action_id,
        action_schema_version: c.action_schema_version,
        observed_view_revision: c.observed_view_revision,
        preconditions: Some(core::Preconditions {
            target_version: c.preconditions.target_version,
            text_version: c.preconditions.text_version,
            model_version: c.preconditions.model_version,
            focus_generation: c.preconditions.focus_generation,
        }),
        arguments: Some(arguments),
    }
}
fn deliver(tx: &SyncSender<UiEvent>, pending: &mut Option<UiEvent>) -> bool {
    if let Some(event) = pending.take() {
        match tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => *pending = Some(event),
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}
fn status(tx: &SyncSender<UiEvent>, text: &'static str) {
    let _ = tx.try_send(UiEvent::Status(text));
}
fn request_id(counter: &mut u64) -> Option<[u8; 16]> {
    let value = *counter;
    *counter = value.checked_add(1)?;
    let mut id = [0; 16];
    id[..8].copy_from_slice(b"RUIreq01");
    id[8..].copy_from_slice(&value.to_be_bytes());
    Some(id)
}

/// Runs on a Tokio worker, never GTK's main thread. select! requires the
/// transport's cancellation-safe receive contract, including retained acks.
pub async fn worker(
    options: Options,
    mut commands: mpsc::Receiver<UiCommand>,
    mut viewport: watch::Receiver<Viewport>,
    events: SyncSender<UiEvent>,
    ui: watch::Sender<UiState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ui_state = UiState::default();
    let mut profiles = SessionConfig::default().supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    profiles.insert(CURSOR_PROFILE.into());
    let hello = ClientHello {
        client_instance_id: options.client_id,
        supported_versions: [1].into_iter().collect(),
        supported_profiles: profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    let mut client = match NetworkClient::connect(
        options.address,
        options.server_name,
        options.certificate,
        &options.credential,
        hello,
    )
    .await
    {
        Ok(c) => c,
        Err(_) => {
            status(
                &events,
                "Connection failed; check approval and pinned identity.",
            );
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let budget = client.receiver().memory_budget();
    let _queue_memory = match budget.reserve(32 * std::mem::size_of::<UiCommand>() + 8192) {
        Ok(m) => m,
        Err(_) => {
            status(&events, "Memory limit reached.");
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let renderer = match Renderer::new_with_profile(
        RenderLimits::default(),
        budget.clone(),
        RenderProfile {
            cursor_vectors: client
                .profiles
                .iter()
                .any(|profile| profile == CURSOR_PROFILE),
        },
    ) {
        Ok(r) => r,
        Err(_) => {
            status(&events, "Pinned renderer is unavailable.");
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut pending = Some(UiEvent::Ready(budget.clone()));
    if client.open_view().await.is_err() {
        status(&events, "View could not be opened.");
        ui_state.clear(Lifecycle::Closed);
        ui.send_replace(ui_state);
        return;
    }
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ping = tokio::time::interval(Duration::from_secs(2));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = false;
    let mut serial = 0u64;
    let mut issued = None;
    let mut presented: Option<Stamp> = None;
    let mut lease: Option<Lease> = None;
    let mut sequence = 1u64;
    let mut selected = None;
    let mut auto_acquired = false;
    let mut presented_frames = 0u64;
    let mut requests = 1u64;
    loop {
        ui_state.editing = match (selected, lease.as_ref(), presented) {
            (Some(target), Some(current), Some(stamp))
                if ui_state.accepts(stamp)
                    && current
                        .wire
                        .allowed_actions
                        .iter()
                        .any(|a| a == "ReplaceText") =>
            {
                current
                    .wire
                    .lease_id
                    .as_slice()
                    .try_into()
                    .ok()
                    .and_then(|lease_id| {
                        client.receiver().state().ok().and_then(|s| {
                            interaction::editing_context(
                                s,
                                target,
                                &CommandBinding {
                                    epoch: client.epoch,
                                    view: client.view_id,
                                    client: options.client_id,
                                    lease: lease_id,
                                    generation: current.wire.generation,
                                    sequence,
                                },
                            )
                        })
                    })
            }
            _ => None,
        };
        if *ui.borrow() != ui_state {
            ui.send_replace(ui_state);
        }
        if !deliver(&events, &mut pending) {
            break;
        }
        tokio::select! {
            _=shutdown.changed()=>{ui_state.clear(Lifecycle::Quit);break;},
            _=tick.tick()=>{
                if dirty && pending.is_none() {
                    let v=*viewport.borrow();
                    match client.receiver().state().map_err(|_|()).and_then(|state|renderer.render_view(state,v,client.receiver()).map_err(|_|())) {
                        Ok(frame)=>match frame.into_rgba8_over(Color{red:1.0,green:1.0,blue:1.0,alpha:1.0}) {
                            Ok(pixels)=>{if !ui_state.resume() {break;}ui.send_replace(ui_state);serial=match serial.checked_add(1) {Some(v)=>v,None=>break};let stamp=Stamp{generation:ui_state.generation,serial,revision:pixels.revision,viewport:pixels.viewport};issued=Some(stamp);pending=Some(UiEvent::Frame{stamp,pixels});dirty=false;},
                            Err(_)=>{pending=None;ui_state.clear(Lifecycle::Suspended);status(&events,"Display allocation limit reached.");dirty=false;},
                        },
                        Err(_)=>{pending=None;ui_state.clear(Lifecycle::Suspended);status(&events,"Scene cannot be rendered within this profile and its limits.");dirty=false;},
                    }
                }
            },
            _=ping.tick()=>{
                if client.send(session::Kind::Ping,&session::Ping{nonce:serial}).await.is_err() {break;}
                if let Some(current)=&lease {
                    let Some(request)=request_id(&mut requests) else {break;};
                    if client.send(session::Kind::RenewControl,&session::RenewControl{request_id:request.to_vec(),lease_id:current.wire.lease_id.clone(),generation:current.wire.generation}).await.is_err() {break;}
                }
            },
            changed=viewport.changed()=>{if changed.is_err() {break;}dirty=true;},
            event=client.receive()=>match event {
                Ok(ClientEvent::Applied{..})=>{dirty=true;status(&events,"View applied; waiting for display.");},
                Ok(ClientEvent::ControlGranted(granted))=>{
                    let charge=2048+granted.allowed_actions.capacity()*std::mem::size_of::<String>()+granted.allowed_actions.iter().map(String::capacity).sum::<usize>()+granted.lease_id.capacity()+granted.request_id.capacity();
                    match budget.reserve(charge) {Ok(memory)=>{lease=Some(Lease{wire:granted,_memory:memory});status(&events,"Control granted.");if options.smoke_frames.is_some_and(|n|presented_frames>=n) {ui_state.clear(Lifecycle::Quit);break;}},Err(_)=>break}
                },
                Ok(ClientEvent::ControlRevoked(_))=>{lease=None;selected=None;status(&events,"Observing; control released.");},
                Ok(ClientEvent::CommandResult(result))=>{status(&events,if result.status=="Applied" || result.status=="APPLIED" {"Remote action applied."} else {"Remote action completed or rejected."});},
                Ok(ClientEvent::ResetRequired(_))=>{status(&events,"View reset required; reconnect.");break;},
                Ok(ClientEvent::Error(_))=>status(&events,"Host rejected the request."),
                Ok(_)=>{},
                Err(_)=>{status(&events,"Disconnected; input has been cleared.");break;},
            },
            command=commands.recv()=>{
                let Some(command)=command else {break;};
                match command {
                    UiCommand::Quit=>{ui_state.clear(Lifecycle::Quit);break;},
                    UiCommand::Acquire=>{let Some(request)=request_id(&mut requests) else {break;};if client.acquire_control(request).await.is_err() {break;}},
                    UiCommand::Presented(stamp)=>{
                        if ui_state.accepts(stamp) && issued.is_some_and(|s:Stamp|s.serial==stamp.serial && s.revision==stamp.revision)
                            && client.receiver().state().is_ok_and(|s|s.revision==stamp.revision) {
                            if client.presented(stamp.revision).await.is_err() {break;}
                            presented=Some(stamp);presented_frames+=1;status(&events,"Displayed.");
                            if options.auto_control && !auto_acquired {auto_acquired=true;let Some(request)=request_id(&mut requests) else {break;};if client.acquire_control(request).await.is_err() {break;}}
                            if options.smoke_frames.is_some_and(|n|presented_frames>=n) && (!options.auto_control || lease.is_some()) {ui_state.clear(Lifecycle::Quit);break;}
                        } else {dirty=true;}
                    },
                    input=>{
                        let stamp=match &input {UiCommand::Click{stamp,..}|UiCommand::Commit{stamp,..}|UiCommand::SelectAll{stamp}=>*stamp,_=>unreachable!()};
                        if matches!(&input,UiCommand::Click{..}) {selected=None;ui_state.editing_generation=match ui_state.editing_generation.checked_add(1){Some(n)=>n,None=>break};}
                        if !ui_state.accepts(stamp) || presented.is_none() || (!matches!(&input,UiCommand::Commit{..}) && (!presented.is_some_and(|p|p.serial==stamp.serial) || !client.receiver().state().is_ok_and(|s|s.revision==stamp.revision))) {status(&events,"View changed; wait for the latest frame.");continue;}
                        let state=match client.receiver().state() {Ok(s)=>s,Err(_)=>continue};
                        let target=match input {
                            UiCommand::Click{point,..}=>{match interaction::hit_test(state,stamp.revision,stamp.viewport,point) {Ok(t)=>{selected=Some(t.semantic);t.semantic},Err(_)=>{selected=None;status(&events,"No enabled remote action here.");continue;}}},
                            UiCommand::Commit{context,..}=>context.target,
                            _=>match selected {Some(t)=>t,None=>{status(&events,"Select a remote text target first.");continue;}},
                        };
                        let Some(Record::Semantic(node))=state.records.get(&target) else {continue;};
                        let mut input_memory=None;
                        let mut captured=None;
                        let (action,args)=match input {
                            UiCommand::Click{..}=>{
                                if node.actions.iter().any(|a|a.id=="Focus") && node.actions.iter().any(|a|a.id=="ReplaceText") {("Focus",Arguments::None)}
                                else if node.actions.iter().any(|a|a.id=="Activate") {("Activate",Arguments::None)}
                                else if node.actions.iter().any(|a|a.id=="SetValue") {
                                    let Some(TypedValue::Boolean(value))=&node.value else {status(&events,"This value requires an explicit editor.");continue;};("SetValue",Arguments::Checked(!value))
                                } else {status(&events,"Text target selected; type to edit or Ctrl+A to select all.");continue;}
                            },
                            UiCommand::Commit{text,_memory,context,..}=>{
                                input_memory=Some(_memory);
                                if ui_state.editing != Some(context) || selected != Some(context.target) {status(&events,"Editing context changed; composition cancelled.");continue;}
                                captured=Some(context);
                                ("ReplaceText",Arguments::ReplaceText{start:context.selection.anchor.min(context.selection.focus),end:context.selection.anchor.max(context.selection.focus),text})
                            },
                            UiCommand::SelectAll{..}=>{let Some(reference)=node.text else {continue;};let Some(Record::Text(model))=state.records.get(&reference.id) else {continue;};let Some(content)=&model.content else {continue;};("SetSelection",Arguments::Selection{anchor:0,focus:content.chars().count() as u64})},
                            _=>unreachable!(),
                        };
                        let Some(current)=&lease else {status(&events,"Request control to send remote input.");continue;};
                        if !current.wire.allowed_actions.iter().any(|a|a==action) {status(&events,"This action is not granted.");continue;}
                        let lease_id: [u8;16]=match current.wire.lease_id.as_slice().try_into() {Ok(id)=>id,Err(_)=>break};
                        if captured.is_some_and(|context|interaction::editing_context(state,target,&CommandBinding{epoch:client.epoch,view:client.view_id,client:options.client_id,lease:lease_id,generation:current.wire.generation,sequence}) != Some(context)) {status(&events,"Editing context changed; composition cancelled.");continue;}
                        let mut domain=match interaction::command(state,target,action,args,CommandBinding{epoch:client.epoch,view:client.view_id,client:options.client_id,lease:lease_id,generation:current.wire.generation,sequence}) {Ok(c)=>c,Err(_)=>{status(&events,"Remote target or editing context is unavailable.");continue;}};
                        if let Some(context)=captured {domain.observed_view_revision=context.revision;}
                        let wire=wire_command(domain);let length=wire.encoded_len();let _memory=match budget.reserve(length+8192) {Ok(m)=>m,Err(_)=>break};
                        let mut bytes=Vec::new();if bytes.try_reserve_exact(length).is_err() || wire.encode(&mut bytes).is_err() {break;}
                        if client.submit(&bytes).await.is_err() {break;}sequence=match sequence.checked_add(1) {Some(n)=>n,None=>break};
                        drop(input_memory);
                    },
                }
            },
        }
    }
    drop(pending);
    ui_state.clear(Lifecycle::Closed);
    ui.send_replace(ui_state);
    let _ = client.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_watch_survives_full_frame_queue_and_is_terminal() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        assert!(tx.try_send(UiEvent::Status("occupied")).is_ok());
        let (state_tx, state_rx) = watch::channel(UiState::default());
        let old = Stamp {
            generation: 0,
            serial: 1,
            revision: 1,
            viewport: Viewport::new(1, 1),
        };
        let mut state = UiState::default();
        state.clear(Lifecycle::Closed);
        state_tx.send_replace(state);
        assert_eq!(state_rx.borrow().lifecycle, Lifecycle::Closed);
        assert!(!state_rx.borrow().accepts(old));
        assert!(!state.resume());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::Status("occupied"))));
        let mut state = UiState::default();
        state.clear(Lifecycle::Quit);
        state.clear(Lifecycle::Closed);
        assert!(!state.resume());
        assert_eq!(state.lifecycle, Lifecycle::Quit);
    }
    #[test]
    fn resumed_render_rejects_every_queued_old_generation() {
        let old = Stamp {
            generation: 0,
            serial: 1,
            revision: 1,
            viewport: Viewport::new(1, 1),
        };
        let mut state = UiState::default();
        state.clear(Lifecycle::Suspended);
        assert!(!state.accepts(old));
        assert!(state.resume());
        assert!(!state.accepts(old));
        assert!(state.accepts(Stamp {
            generation: state.generation,
            ..old
        }));
    }
    #[test]
    fn acquisition_and_renewal_request_ids_are_fresh_until_checked_exhaustion() {
        let mut counter = 1;
        let first = request_id(&mut counter).unwrap();
        let second = request_id(&mut counter).unwrap();
        assert_ne!(first, second);
        assert_eq!(&first[8..], &1u64.to_be_bytes());
        let mut counter = u64::MAX;
        assert_eq!(request_id(&mut counter), None);
    }
}
