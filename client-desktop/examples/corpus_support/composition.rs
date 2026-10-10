//! Actual native IM callbacks -> advertised wire family -> causal source edit.
use super::*;
fn document(path: &Path) -> Result<Value> {
    let artifact = CaptureArtifact::read(path)?;
    artifact.manifest()["semantics"]
        .as_array()
        .ok_or("source semantics")?
        .iter()
        .find(|node| node["label"] == "Modal entry")
        .map(|node| node["text"].clone())
        .ok_or("native modal TextModel absent".into())
}
async fn capture_pair(
    source: &Path,
    vector: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    budget: &MemoryBudget,
    diagnostics: &Diagnostics,
) -> Result<(Value, Value)> {
    let path = retain(&latest(source)?, vector)?;
    let controller = compare(&path, vector, &observations[0], budget, diagnostics).await?;
    let observer = compare(&path, vector, &observations[1], budget, diagnostics).await?;
    Ok((json!([controller.proof, observer.proof]), document(&path)?))
}
pub async fn drive(
    root: &Path,
    directory: &Path,
    source: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    diagnostics: &Diagnostics,
    budget: &MemoryBudget,
    initial: (Compared, Compared),
) -> Result<Value> {
    let vector = directory.join("source-vector");
    let point = initial
        .0
        .editable
        .ok_or("source-owned modal editable Binding absent")?;
    native(
        root,
        CONTROLLER,
        "click",
        &[point.x.to_string(), point.y.to_string()],
    )?;
    wait(&observations[0], |o| {
        o.wire_composition
            && !o.gtk_composition_clock
            && o.input_ready
            && o.editing_target.is_some()
            && o.displayed.is_some_and(|d| d.stamp.revision == 2)
    })
    .await?;
    let (focused, focused_text) =
        capture_pair(source, &vector, observations, budget, diagnostics).await?;
    if observations[1].borrow().wire_composition {
        return Err("observer unexpectedly owns native composition".into());
    }
    native(root, CONTROLLER, "keys", &["a".into()])?;
    wait(&observations[0], |o| {
        o.input_ready
            && o.typing_pending == 0
            && o.composition.commits == 1
            && o.displayed.is_some_and(|d| d.stamp.revision == 3)
    })
    .await?;
    let (ascii, ascii_text) =
        capture_pair(source, &vector, observations, budget, diagnostics).await?;
    if ascii_text["content"] != "a" || ascii_text["selection"] != json!([1, 1]) {
        return Err("native ASCII commit/caret differs".into());
    }
    let ascii_callbacks = observations[0].borrow().composition;
    if ascii_callbacks.begins != 1 || ascii_callbacks.updates != 0 || ascii_callbacks.cancels != 0 {
        return Err("ASCII unexpectedly used preedit/fallback".into());
    }
    native(
        root,
        CONTROLLER,
        "keys",
        &[
            "+Control_L",
            "+Shift_L",
            "u",
            "-Shift_L",
            "-Control_L",
            "0",
            "0",
            "e",
            "9",
        ]
        .map(str::to_owned),
    )?;
    let preedit = wait(&observations[0], |o| {
        o.preedit_visible && o.composition.updates > 0 && o.typing_pending == 0
    })
    .await?;
    let private_text = document(&latest(source)?)?;
    let observer = *observations[1].borrow();
    if private_text != ascii_text
        || preedit.composition.commits != 1
        || observer.preedit_visible
        || observer.displayed.is_none_or(|d| d.stamp.revision != 3)
    {
        return Err("transient preedit changed authoritative/observer text or publication".into());
    }
    native(root, CONTROLLER, "keys", &["Return".into()])?;
    wait(&observations[0], |o| {
        !o.preedit_visible
            && o.input_ready
            && o.typing_pending == 0
            && o.composition.commits == 2
            && o.displayed.is_some_and(|d| d.stamp.revision == 4)
    })
    .await?;
    let (unicode, unicode_text) =
        capture_pair(source, &vector, observations, budget, diagnostics).await?;
    if unicode_text["content"] != "aé" || unicode_text["selection"] != json!([2, 2]) {
        return Err("native Unicode composition did not commit once at scalar caret".into());
    }
    // Two keys in one native event batch must preserve scalar order and
    // causally rebase. The portable queue regression separately proves overlap
    // before a source roundtrip; this batch alone does not measure overlap.
    native(root, CONTROLLER, "keys", &["x".into(), "y".into()])?;
    wait(&observations[0], |o| {
        o.input_ready
            && o.typing_pending == 0
            && o.composition.commits == 4
            && o.displayed.is_some_and(|d| d.stamp.revision == 6)
    })
    .await?;
    let (rapid, rapid_text) =
        capture_pair(source, &vector, observations, budget, diagnostics).await?;
    if rapid_text["content"] != "aéxy" || rapid_text["selection"] != json!([4, 4]) {
        return Err("rapid native commits lost scalar order/caret".into());
    }
    native(
        root,
        CONTROLLER,
        "keys",
        &[
            "+Control_L",
            "+Shift_L",
            "u",
            "-Shift_L",
            "-Control_L",
            "0",
            "0",
            "e",
            "9",
        ]
        .map(str::to_owned),
    )?;
    wait(&observations[0], |o| {
        o.preedit_visible && o.typing_pending == 0
    })
    .await?;
    native(root, CONTROLLER, "keys", &["Escape".into()])?;
    let cancelled = wait(&observations[0], |o| {
        !o.preedit_visible && o.composition.cancels == 1 && o.typing_pending == 0
    })
    .await?;
    if document(&latest(source)?)? != rapid_text || cancelled.composition.commits != 4 {
        return Err("native Escape mutated document/committed cancelled preedit".into());
    }
    // Retain the complete original scope before the explicit priority stop
    // causes this bounded GTK adapter to negotiate a fresh observer view.
    let controller_trace = export(
        diagnostics,
        &observations[0],
        directory,
        "controller",
        budget,
    )?;
    let before_control_stop = observations[0]
        .borrow()
        .binding
        .ok_or("controller scope")?
        .view;
    native(
        root,
        CONTROLLER,
        "keys",
        &["+Control_L", "Escape", "-Control_L"].map(str::to_owned),
    )?;
    let stopped = wait(&observations[0], |o| {
        !o.has_control && o.binding.is_some_and(|b| b.view != before_control_stop) && o.input_ready
    })
    .await?;
    if document(&latest(source)?)? != rapid_text {
        return Err("priority reset changed authoritative text".into());
    }
    Ok(json!({
        "mode":"actual-native-gtk-wire-composition-mirror",
        "presentation_scope":"actual GTK Canvas snapshot/after-paint before every admitted wire stage and before rebased next composition; no simulated Presented",
        "source_scope":"cooperating GTK4.22.4 native editable, guarded private transient state, common ledger and scalar commit; exact focus-Surface causal token and canonical public transaction clocks",
        "selected_clock_profile":null,
        "clock_policy":"standard-public-clocks",
        "clock_profile_evidence":"Actual client observes the native GTK clock profile absent; recorder excludes credential handshake/ServerHello.",
        "observer_scope":"separate Rust receiver/renderer instance; transient preedit never publishes document content",
        "initial":[initial.0.proof,initial.1.proof],"focused":focused,"focused_text":focused_text,
        "ascii":ascii,"ascii_text":ascii_text,"unicode":unicode,"unicode_text":unicode_text,
        "rapid_native_commits":rapid,"rapid_text":rapid_text,
        "preedit_privacy":{"document_unchanged":true,"observer_revision":3,"observer_private_preedit_visible":false},
        "native_callbacks":{"begins":cancelled.composition.begins,"updates":cancelled.composition.updates,"commits":cancelled.composition.commits,"cancels":cancelled.composition.cancels},
        "escape":{"document_unchanged":true,"cancelled_preedit":true},
        "priority_reset":{"fresh_observer_scope":hex(&stopped.binding.unwrap().view),"has_control":stopped.has_control},
        "controller_trace":controller_trace,
        "observer_trace":export(diagnostics,&observations[1],directory,"observer",budget)?,
    }))
}
