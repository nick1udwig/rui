"""Bounded opt-in monotonic source measurements, without captured content."""
from collections import deque
import json
import os
from pathlib import Path
import time


class Timings:
    def __init__(self, capacity=4096):
        if not 1 <= capacity <= 16384:
            raise ValueError('timing capacity limit')
        self.samples = deque(maxlen=capacity)
        self.dropped = 0

    def record(self, stage, revision, started=None, count=1):
        if len(self.samples) == self.samples.maxlen:
            self.dropped += 1
        self.samples.append({'stage': stage, 'revision': str(revision),
                             'durationMs': 0 if started is None else (time.monotonic_ns()-started)/1e6,
                             'count': count})

    def report(self):
        return {'capacity': self.samples.maxlen, 'dropped': self.dropped, 'samples': list(self.samples)}


capacity = os.environ.get('RUI_TIMINGS')
timings = Timings(min(16384, max(1, int(capacity)))) if capacity else None


def write_report():
    path = os.environ.get('RUI_SOURCE_TIMINGS_REPORT')
    if timings and path:
        Path(path).write_text(json.dumps(timings.report())+'\n')
