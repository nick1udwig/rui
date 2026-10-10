"""Read-only observed application view with explicit age/scope/availability.

Use one worker per authorized application to isolate stalls. No input is exposed.
The public envelope remains available for a validated Rust/WASM semantic client.
"""
import html
import time
from adapters.atspi.observer import ObservedProjection


class ReadOnlyObservedView:
    def __init__(self,port,*,clock=time.monotonic,max_age=2):
        if not 0 < max_age <= 60:raise ValueError('observation age ceiling')
        self.port,self.clock,self.max_age=port,clock,max_age
        self.projection=ObservedProjection()
        self.sources={};self.sampled_at=None;self.unavailable=True;self.closed=False
        self.pending_scope=None

    def scope(self,sources):
        if self.closed or len(sources)>1:raise ValueError('one authorized source per isolated view')
        if any(type(pid) is not int or not 0 < pid < 1<<63 or not isinstance(inc,str) or len(inc)!=32 or any(c not in '0123456789abcdef' for c in inc) for pid,inc in sources.items()):
            raise ValueError('trusted process/incarnation scope required')
        if sources==self.sources:return
        # The owner must finish or replace an old worker before changing scope;
        # a late reply is never associated with the new process incarnation.
        if self.port.pending is not None:self.port.abort()
        self.sources=dict(sources);self.pending_scope=None;self.sampled_at=None;self.unavailable=True
        self.projection.publish({'type':'snapshot','classification':'OBSERVED','apps':[]},{})

    def refresh(self):
        if self.closed or not self.sources:raise ValueError('authorized source required')
        self.port.request('crawl',pids=list(self.sources))
        self.pending_scope=dict(self.sources)

    def poll(self):
        if self.closed:return False
        result=self.port.poll()
        if result is None:return False
        if result.get('type')!='snapshot' or self.pending_scope!=self.sources:
            self.unavailable=True;self.pending_scope=None;return True
        try:
            self.projection.publish(result,self.sources)
        except (ValueError,KeyError):
            self.unavailable=True;self.pending_scope=None;raise
        self.sampled_at=self.clock();self.unavailable=False;self.pending_scope=None;return True

    @property
    def availability(self):
        if self.unavailable:return 'unavailable'
        if self.sampled_at is None or self.clock()-self.sampled_at>self.max_age:return 'stale'
        return 'observed'

    def document(self):
        status=self.availability
        parts=['<!doctype html><meta charset="utf-8"><title>Observed application</title>',
               '<main><h1>Observed application</h1><p role="status">OBSERVED — '+status+'</p>',
               '<p>Partial accessibility hierarchy. Visual association unavailable. Control unavailable.</p>']
        for surface in self.projection.last_metadata or []:
            nodes={node['id']:node for node in surface['nodes']}
            def render(identity,depth=0):
                if depth>16:raise ValueError('view depth ceiling')
                node=nodes[identity]
                name='Private content' if node['availability']=='redacted' else node['name'] or 'Unnamed object'
                parts.append('<li data-id="'+str(identity)+'"><span>'+html.escape(name)+'</span> <small>'+node['availability']+'</small>')
                if node['children']:
                    parts.append('<ul>')
                    for child in node['children']:render(child,depth+1)
                    parts.append('</ul>')
                parts.append('</li>')
            parts.append('<ul>');render(surface['semantic_root']);parts.append('</ul>')
        parts.append('</main>');result=''.join(parts)
        if len(result.encode())>256<<10:raise ValueError('read-only DOM artifact ceiling')
        return result

    def close(self):
        if self.closed:return
        self.closed=True;self.sources={};self.pending_scope=None;self.sampled_at=None;self.unavailable=True
        self.projection.last_metadata=None;self.projection.last_envelope=None
        self.port.close()
