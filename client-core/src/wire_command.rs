use rui_protocol_core::{commands::Arguments, wire::proto as core};
pub fn wire_command(c: rui_protocol_core::commands::SemanticCommand) -> core::SemanticCommand {
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
