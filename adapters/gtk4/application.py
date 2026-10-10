"""Application-facing registered GTK capture, helpers and source-owned topology.

Only an inherited private host connection may instantiate NativeDispatcher.
Applications supply topology from trusted native objects, never peer labels.
"""
from dataclasses import dataclass
from pathlib import Path
import hashlib
import json
from gi.repository import Gtk, GLib
from adapters.sdk.producer import CoherentSource, encode_snapshot, MAX_MANIFEST_BYTES
from adapters.gtk4.capture import CaptureWidget
from adapters.gtk4.state import NativeRegistry, editable_text, invalidate_native_context, require_native_context
from adapters.gtk4.transaction import CaptureTransaction, CaptureBudget, ArtifactQuota, BarrierRejected
from adapters.gtk4.view import CoherentView


@dataclass
class GtkApplicationSource:
    app: object
    source: CoherentSource
    registry: NativeRegistry
    capture: object
    output: Path

    def __post_init__(self):
        if self.registry.source is not self.source or self.capture.source is not self.source:
            raise ValueError('application source identity mismatch')


class Controls:
    """Explicit stable keys plus trusted widgets; no discovery by label."""
    def __init__(self, source, registry, surface):
        self.source, self.registry, self.surface = source, registry, surface

    def label(self, key, widget, parent):
        self.source.register(key,6,lambda:{'label':widget.get_text()},parent=parent)
        widget.connect('notify::label',lambda *_:self.source.changed())

    def button(self, key, widget, parent):
        self.source.register(key,3,lambda:{'label':widget.get_label(),'states':{1:1 if widget.is_sensitive() else 2}},parent=parent,actions=['Activate','Focus'])
        self.registry.bind(key,widget,self.surface)

    def checkbox(self, key, widget, parent):
        if not isinstance(widget, (Gtk.ToggleButton, Gtk.CheckButton)):
            raise ValueError('registered boolean widget required')
        self.source.register(key,4,lambda:{'label':widget.get_label(),'value':bool(widget.get_active()),
            'states':{1:1 if widget.is_sensitive() else 2,2:2 if widget.get_active() else 1}},
            parent=parent,actions=['SetValue','Focus'])
        self.registry.bind(key,widget,self.surface)
        widget.connect('toggled',lambda *_:self.source.changed())

    def entry(self, key, widget, parent, label, *, private=False):
        def read():
            if private:
                return {'label':label,'text':{'content':None,'status':3,'editable':False,'selectable':False}}
            return {'label':label,'text':editable_text(widget),'states':{1:1 if widget.is_sensitive() else 2,5:1 if widget.get_editable() else 2}}
        self.source.register(key,5,read,parent=parent,actions=[] if private else ['ReplaceText','SetSelection','Focus'])
        self.registry.bind(key,widget,self.surface)

    def collection(self, key, widget, parent, read):
        self.source.register(key,7,read,parent=parent,actions=['ScrollTo'])
        self.registry.bind(key,widget,self.surface)

    def item(self, key, widget, parent, label):
        self.source.register(key,8,lambda:{'label':label(),'states':{3:2 if widget.is_selected() else 1}},parent=parent,actions=['Activate','Focus'])
        self.registry.bind(key,widget,self.surface)


class RegisteredApplication(CoherentView):
    """Bounded union of <=4 explicitly registered content surfaces.

    Logical placement belongs to this source integration. It is not inferred
    from a compositor. Parent predicates must read the registered native objects.
    """
    def __init__(self, app, source, registry, output, *, size=(720,600), diagnostics=False):
        if not 0 < size[0]*size[1] <= 1_048_576:
            raise ValueError('logical canvas ceiling')
        self.app,self.source,self.registry,self.output = app,source,registry,Path(output)
        self.size,self.diagnostics = size,diagnostics
        self.captures,self.descriptors = {},{}
        self.consumer,self.command_cause = None,None
        self.stage='registered-application'
        self.revision,self.focus_generation,self.previous_focus = 0,1,None
        self.budget=CaptureBudget(); source.capture_budget=self.budget
        source.artifact_quota=ArtifactQuota(limit=128 << 20,max_files=16384)
        self.view_id=source.identifier('whole-view'); self.session_id=source.identifier('whole-view-session')
        self.barrier=CaptureTransaction(source,self.oracle,self.request,self.commit,budget=self.budget,timeout=3)
        settings=Gtk.Settings.get_default()
        settings.set_property('gtk-enable-animations',False);settings.set_property('gtk-cursor-blink',False)
        GLib.timeout_add(50,self.deadline_tick)

    def surface(self, key, capture, window, *, origin=(0,0), stacking=0, parent=None, modal=False, parent_matches):
        if len(self.captures)>=4 or key in self.captures or capture.source is not self.source:
            raise ValueError('registered surface ceiling or identity')
        if parent is not None and parent not in self.captures:
            raise ValueError('register parent before child')
        capture.diagnostics=self.diagnostics
        self.captures[key]=capture
        self.descriptors[key]={'origin':list(origin),'stack':stacking,'parent':parent,'modal':modal,'matches':parent_matches,'window':window}
        self.registry.surface(key,capture,window)
        capture.capture_transaction=self.barrier; self.barrier.register(key,capture)
        capture.connect('map',lambda *_:self.source.changed())
        capture.connect('unmap',lambda *_,c=capture:c.publish_lifecycle())
        for signal in ('map','unmap'): capture.connect(signal,self.observe_focus)
        window.connect('notify::focus-widget',self.observe_focus)
        self.source.identifier('surface:'+key)

    def integration(self, main):
        return GtkApplicationSource(self.app,self.source,self.registry,self.captures[main],self.output)

    def observe_focus(self,*_):
        try:
            require_native_context(self.source)
            mapped=[key for key,c in self.captures.items() if c.source_alive and c.get_mapped()]
            modals=[key for key in mapped if self.descriptors[key]['modal']]
            focus=None
            for key in sorted(modals or mapped,key=lambda k:self.descriptors[k]['stack'],reverse=True):
                target=self.registry.focus(key)
                if target is not None: focus=(key,self.source.identifier('semantic:'+target));break
            if focus!=self.previous_focus:
                if self.focus_generation >= (1<<64)-1: raise BarrierRejected('native-focus-generation-ceiling')
                self.previous_focus=focus;self.focus_generation+=1;self.source.changed()
            return focus
        except Exception:
            invalidate_native_context(self.source)
            return None

    def oracle(self):
        require_native_context(self.source)
        focus=self.observe_focus();require_native_context(self.source)
        surfaces={}
        for key,c in self.captures.items():
            d=self.descriptors[key];alive=c.source_alive;mapped=alive and c.get_mapped()
            matches=bool(d['matches']())
            if alive and not matches:raise BarrierRejected('registered-native-parent-changed')
            parent=self.source.identifier('surface:'+d['parent']) if d['parent'] else None
            if d['modal'] and mapped and not d['window'].get_modal():raise BarrierRejected('native-modality-changed')
            surfaces[key]={'alive':alive,'mapped':mapped,'native_parent_matches':matches,
                'surface_id':self.source.identifier('surface:'+key),'semantic_root':self.source.identifier('semantic:'+c.semantic_root),
                'size':[c.get_width(),c.get_height()],'origin':d['origin'],'stacking_order':d['stack'],
                'parent':parent,'modal_for':parent if d['modal'] and mapped else None,'native_modal':d['modal'],
                'semantics':self.source.semantics(c.semantic_root,1) if alive else [],'binding_regions':self.source.binding_regions(key)}
        return {'incarnation':self.source.identity,'source_revision':self.source.source_revision,'surfaces':surfaces,
                'focus':focus,'focus_generation':self.focus_generation,
                'visual_policy':{'animations':False,'cursor_blink':False,'controlled_static_canvas':True}}

    def request(self,nonce,required):
        def queue():
            if self.barrier.active and self.barrier.nonce==nonce:
                for key in sorted(required):self.captures[key].queue_draw()
            return False
        GLib.idle_add(queue)

    def commit(self,nonce,facts,candidates,offers):
        self.revision+=1;frame=self.assemble(nonce,facts,candidates,offers)
        frame['source_generation']=getattr(self.source,'private_generation',1)
        if self.command_cause is not None:frame['command_cause']=self.command_cause
        binding={'session_id':'01'*16,'epoch':self.source.identity,'view_id':f'{self.view_id:032x}'}
        envelope=encode_snapshot(frame,binding)
        frame.update(binding=binding,envelope_sha256=hashlib.sha256(envelope).hexdigest())
        frame['protocol_handoff']={'envelope_bytes':len(envelope),'resource_bytes':sum(x['total_bytes'] for x in offers.values()),
            'resource_kind_counts':{str(kind):sum(x['kind']==kind for x in offers.values()) for kind in (1,2)}}
        label=f'whole-{self.revision:04d}';frame['surface_key']='whole'
        if self.diagnostics:self.local_reference(label,frame)
        manifest=json.dumps(frame,sort_keys=True,indent=2).encode()
        if len(manifest)>MAX_MANIFEST_BYTES:raise BarrierRejected('whole-view-manifest-ceiling')
        self.source.write_artifact(self.output,label+'.bin',envelope)
        self.source.write_artifact(self.output,label+'.json',manifest)
        if self.consumer is None:raise BarrierRejected('whole-source-consumer-missing')
        self.consumer(self,frame,label)

    def failed(self,error):
        self.barrier.close();self.app.quit()

    def deadline_tick(self):
        if self.barrier.closed:return False
        if self.barrier.expire():self.failed(BarrierRejected('whole-view-native-deadline'));return False
        return True
