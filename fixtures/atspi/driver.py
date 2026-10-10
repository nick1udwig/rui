#!/usr/bin/python3
"""Actual isolated accessibility reconciliation, interruption and restart proof."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from adapters.atspi.observer import ObservedProjection, WorkerPort
from adapters.atspi.view import ReadOnlyObservedView


class Source:
    def __init__(self, name):
        self.name = name
        self.incarnation = uuid.uuid4().hex
        self.channel, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.channel.settimeout(3)
        try:
            self.process = subprocess.Popen(['/usr/bin/python3', str(Path(__file__).with_name('source.py')),
                                            '--fd', str(child.fileno()), '--identity', name], pass_fds=(child.fileno(),))
        finally:
            child.close()
        assert json.loads(self.channel.recv(4096))['ready']

    def replace(self):
        self.channel.send(b'{"op":"replace"}')
        return json.loads(self.channel.recv(4096))['generation']

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=2)
        self.channel.close()


def main():
    assert os.environ.get('RUI_ISOLATED_DISPLAY') == '1'
    output = Path(os.environ['RUI_ATSPI_OUTPUT'])
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ValueError('empty owned output directory required')
    daemon = subprocess.Popen(['dbus-daemon', '--session', '--nofork', '--print-address=1'], stdout=subprocess.PIPE, text=True)
    registry = None
    sources = []
    worker = None
    views = []
    try:
        address = daemon.stdout.readline().strip()
        os.environ['AT_SPI_BUS_ADDRESS'] = address
        registry = subprocess.Popen(['/usr/lib/at-spi2-registryd'], stdout=subprocess.DEVNULL)
        time.sleep(.15)
        for name in ('A', 'B'):
            sources.append(Source(name))
        worker = WorkerPort(address)
        projection = ObservedProjection()
        report = {'scope': 'owned-private-AT-SPI-bus-and-two-GTK-applications',
                  'classification': 'OBSERVED', 'native_surface_association': 'unresolved',
                  'input_modes': [], 'captures': [], 'files': {}}
        heartbeats = 0
        maximum_poll = 0

        def finish(barrier=None):
            nonlocal heartbeats, maximum_poll
            end = time.monotonic() + 4
            while time.monotonic() < end:
                start = time.monotonic()
                result = worker.poll()
                maximum_poll = max(maximum_poll, time.monotonic() - start)
                heartbeats += 1
                if result:
                    if result['type'] == 'crawl_barrier':
                        assert barrier is not None
                        barrier()
                        worker.resume_fixture_barrier()
                    else:
                        return result
                time.sleep(.002)
            raise TimeoutError('private worker did not finish')

        def crawl(name, barrier=None):
            worker.request('crawl', pids=[s.process.pid for s in sources], fixture_barrier=barrier is not None)
            observation = finish(barrier)
            assert observation['type'] == 'snapshot', observation
            wire, metadata = projection.publish(observation, {s.process.pid: s.incarnation for s in sources})
            file = name + '.bin'
            (output / file).write_bytes(wire)
            report['files'][file] = hashlib.sha256(wire).hexdigest()
            report['captures'].append({'name': name, 'revision': projection.revision,
                                       'surfaces': metadata,
                                       **{key: observation[key] for key in ('reconciliations', 'calls', 'events', 'dropped_events', 'worker_peak_rss_kib')}})
            return observation

        initial = crawl('initial')
        initial_ids = dict(projection.ids)
        a_incarnation = sources[0].incarnation
        a_ids = {value for key, value in initial_ids.items() if key[1] == a_incarnation}
        b_ids = set(initial_ids.values()) - a_ids
        assert len(initial['apps']) == 2
        first, second = [app['frames'][0] for app in initial['apps']]
        assert first['bounds'] == second['bounds']
        assert first['records'][0]['name'] == second['records'][0]['name'] == 'Identical overlapping title'
        assert first['root'][0] != second['root'][0]
        report['ambiguous_windows'] = {'same_title': True, 'same_bounds': True, 'distinct_trusted_process_incarnations': True}

        worker.request('suppress_fixture_events', value=True)
        assert finish()['type'] == 'configured'
        sources[0].replace()
        reconciled = crawl('event-gap')
        assert reconciled['dropped_events'] > 0
        assert any(r['name'] == 'Replacement 1' for app in reconciled['apps'] for frame in app['frames'] for r in frame['records'])
        assert b_ids <= set(projection.ids.values())
        worker.request('suppress_fixture_events', value=False)
        assert finish()['type'] == 'configured'

        interrupted = crawl('crawl-race', sources[0].replace)
        assert interrupted['reconciliations'] >= 1
        assert any(r['name'] == 'Replacement 2' for app in interrupted['apps'] for frame in app['frames'] for r in frame['records'])

        # An unresponsive app blocks only the disposable reader. The owner keeps
        # polling, and the previously validated publication stays byte-identical.
        prior = projection.last_envelope
        before_ticks = heartbeats
        os.kill(sources[0].process.pid, signal.SIGSTOP)
        try:
            worker.request('crawl', pids=[s.process.pid for s in sources])
            unavailable = finish()
        finally:
            os.kill(sources[0].process.pid, signal.SIGCONT)
        assert unavailable['type'] == 'unavailable'
        assert projection.last_envelope == prior
        assert heartbeats - before_ticks >= 20
        report['stalled_source'] = {'owner_poll_iterations': heartbeats - before_ticks, 'previous_publication_unchanged': True,
                                    'result': unavailable['type']}
        crawl('recovered')

        old = sources.pop(0)
        old.close()
        crawl('source-exited')
        assert not a_ids & set(projection.ids.values())
        assert b_ids == set(projection.ids.values())
        sources.insert(0, Source('A'))
        crawl('source-restarted')
        assert not a_ids & set(projection.ids.values())
        assert b_ids <= set(projection.ids.values())
        report['restart'] = {'retired_initial_app_ids': len(a_ids), 'other_app_stable_ids': len(b_ids),
                             'new_incarnation': sources[0].incarnation, 'broker_binding_unchanged': True}
        for source in sources:
            view=ReadOnlyObservedView(WorkerPort(address));views.append(view)
            view.scope({source.process.pid:source.incarnation});view.refresh()
        def finish_views():
            end=time.monotonic()+4
            done=set()
            while len(done)<len(views) and time.monotonic()<end:
                for index,view in enumerate(views):
                    if index not in done and view.poll():done.add(index)
                time.sleep(.002)
            assert len(done)==len(views)
        finish_views()
        assert all(view.availability=='observed' for view in views)
        assert all(b'never export this secret' not in view.projection.last_envelope and 'never export this secret' not in view.document() for view in views)
        os.kill(sources[0].process.pid,signal.SIGSTOP)
        try:
            for view in views:view.refresh()
            finish_views()
            assert views[0].availability=='unavailable' and views[1].availability=='observed'
            report['read_only_views']={'stalled':'unavailable','healthy':'observed','workers':2,'actions':False,'classification':'OBSERVED','passwordContentExcluded':True}
            (output/'healthy-observed.html').write_text(views[1].document())
            (output/'stalled-observed.html').write_text(views[0].document())
            views[0].scope({})
            assert not views[0].projection.last_metadata
            report['read_only_views']['scopeReductionClearsContent']=True
        finally:
            os.kill(sources[0].process.pid,signal.SIGCONT)
        report['limits'] = {'worker_address_space_bytes': 256 << 20, 'message_bytes': 128 << 10,
                            'nodes_per_frame': 128, 'depth': 16, 'calls_per_crawl': 1024,
                            'call_timeout_ms': 200, 'crawl_seconds': 2, 'owner_deadline_seconds': 3,
                            'public_ids_lifetime': 4096, 'source_incarnations': 8, 'in_flight_jobs': 1}
        report['owner_poll_iterations'] = heartbeats
        report['maximum_poll_ms'] = maximum_poll * 1000
        assert maximum_poll < .1
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'passed': True, 'captures': len(report['captures']), 'output': str(output),
                          'dropped_events': reconciled['dropped_events'], 'race_reconciliations': interrupted['reconciliations'],
                          'stalled_owner_polls': report['stalled_source']['owner_poll_iterations'], 'maximum_poll_ms': maximum_poll * 1000}))
    finally:
        for view in views:view.close()
        if worker:
            worker.close()
        for source in sources:
            source.close()
        if registry:
            registry.terminate()
            registry.wait(timeout=2)
        daemon.terminate()
        daemon.wait(timeout=2)


if __name__ == '__main__':
    main()
