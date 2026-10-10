"""Level-triggered dirtiness with a coalesced, inherited datagram wakeup."""
import os
import socket
import struct


class DirtyNotifier:
    def __init__(self, endpoint, incarnation):
        if endpoint.family != socket.AF_UNIX or endpoint.type != socket.SOCK_DGRAM or not 1 <= incarnation < 1 << 64:
            raise ValueError('invalid private dirty channel')
        self.endpoint, self.incarnation = endpoint, incarnation
        endpoint.setblocking(False)
        self.generation = 0
        self.pending = False
        self.closed = False

    @classmethod
    def inherited(cls, incarnation):
        return cls(socket.socket(fileno=int(os.environ['RUI_PRIVATE_DIRTY_FD'])), incarnation)

    def changed(self, generation):
        if self.closed:
            return
        if not self.generation <= generation < 1 << 64:
            raise ValueError('dirty generation overflow or rollback')
        self.generation = generation
        if self.pending:
            return
        try:
            self.endpoint.send(struct.pack('>QQ', self.incarnation, generation))
        except BlockingIOError:
            # A full channel already contains a wakeup. Dirtiness stays in the
            # source, not in this lossy edge. Capture/consumption rechecks it.
            pass
        self.pending = True

    def consumed(self, captured, current):
        if not 0 <= captured <= current < 1 << 64:
            raise ValueError('invalid captured dirty generation')
        self.pending = False
        if current > captured:
            self.changed(current)

    def close(self):
        if not self.closed:
            self.closed = True
            self.endpoint.close()
