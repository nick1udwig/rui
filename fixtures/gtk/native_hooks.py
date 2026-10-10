"""Explicit trusted negative controls for the owned corpus, never application SDK."""
import json
from adapters.gtk4.state import editable_text


class CorpusHooks:
    def __init__(self, corpus):
        self.corpus = corpus

    def handle(self, d, request):
        request_id = request['request_id']
        operation = request['operation']
        if operation == 'race-edit':
            d.race_next = 'edit'
        elif operation == 'race-destroy':
            d.race_next = 'destroy'
        elif operation == 'race-visual':
            d.race_next = 'visual'
        elif operation == 'race-model':
            d.race_next = 'model'
        elif operation == 'race-source-generation':
            d.race_next = 'source-generation'
        elif operation == 'race-focus-cycle' and d.whole is not None:
            d.race_next = 'focus-cycle'
        elif operation == 'selection-cycle' and d.whole is not None:
            widget = self.corpus.registry.widgets['modal-entry'][0]
            before = editable_text(widget)['selection']
            delegate = widget.get_delegate() or widget
            delegate.select_region(0, 1)
            delegate.select_region(*before)
            d.composition.poll()
        elif operation == 'composition-state':
            d.composition.poll()
            state = d.composition.diagnostic()
            state.update(source_generation=d.source_generation, fence_generation=d.fence.current(), native_dispatches=d.native_dispatches, native_callbacks=d.native_callbacks)
            self.corpus.source.write_artifact(self.corpus.output, 'composition-state.json', json.dumps(state).encode())
        elif operation == 'composition-focus-cycle' and d.whole is not None:
            previous = d.whole.modal.get_focus()
            self.corpus.registry.widgets['modal-close'][0].grab_focus()
            d.whole.observe_focus()
            previous.grab_focus()
            d.whole.observe_focus()
            d.composition.poll()
        elif operation == 'composition-remove' and d.whole is not None:
            d.whole.modal_entry.set_visible(False)
            self.corpus.registry.retire('modal-entry')
            d.composition.poll()
        elif operation == 'composition-empty' and d.whole is not None:
            d.whole.modal_entry.set_text('')
            d.composition.poll()
        elif operation == 'interrupt-whole-capture' and d.whole is not None:
            d.whole.interrupt = True
        elif operation in ('lose-after-dispatch', 'stall-after-dispatch', 'uncorrelated-capture', 'crash-after-dispatch'):
            d.fail_after_dispatch = operation
        elif operation == 'delay-until-expired':
            d.delay_until_expired = True
        elif operation == 'delay-for-fence':
            d.delay_for_fence = True
        elif operation == 'disable-button':
            self.corpus.button.set_sensitive(False)
        elif operation == 'enable-button':
            self.corpus.button.set_sensitive(True)
        elif operation == 'destroy-button':
            self.corpus.button.set_visible(False)
            self.corpus.registry.retire('button')
        elif operation == 'destroy-surface':
            d.pending = (request_id, 'fixture-capture')
            self.corpus.capture.retire_source()
            self.corpus.window.destroy()
            return
        else:
            raise ValueError('unsupported-fixture-operation')
        d.reply(request_id, 'fixture')
        return

    def before_dispatch(self, d):
        if d.race_next == 'edit':
            self.corpus.entry.set_text('local intervening source edit')
            d.race_next = None
        elif d.race_next == 'destroy':
            self.corpus.button.set_visible(False)
            self.corpus.registry.retire('button')
            d.race_next = None
        elif d.race_next == 'visual':
            self.corpus.canvas.queue_draw()
            self.corpus.source.changed()
            d.race_next = None
        elif d.race_next == 'focus-cycle':
            d.race_next = None
            previous = d.whole.modal.get_focus()
            other = self.corpus.registry.widgets['modal-close'][0]
            other.grab_focus()
            d.whole.observe_focus()
            previous.grab_focus()
            d.whole.observe_focus()
        elif d.race_next == 'model':
            self.corpus.unmaterialize_row()
            d.race_next = None
