import socket
import struct
import unittest
from adapters.sdk.invalidation import DirtyNotifier


class InvalidationTests(unittest.TestCase):
    def setUp(self):
        self.reader, writer = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
        self.reader.setblocking(False)
        self.notifier = DirtyNotifier(writer, 7)

    def tearDown(self):
        self.reader.close()
        self.notifier.close()

    def read(self):
        return struct.unpack('>QQ', self.reader.recv(16))

    def test_burst_and_change_during_capture(self):
        for generation in range(1, 101):
            self.notifier.changed(generation)
        self.assertEqual(self.read(), (7, 1))
        with self.assertRaises(BlockingIOError):
            self.read()
        self.notifier.consumed(80, 100)
        self.assertEqual(self.read(), (7, 100))
        self.notifier.consumed(100, 100)
        self.notifier.changed(101)
        self.assertEqual(self.read(), (7, 101))

    def test_full_channel_preserves_latest_dirty_state(self):
        self.notifier.endpoint.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 1024)
        while True:
            try:
                self.notifier.endpoint.send(struct.pack('>QQ', 7, 1))
            except BlockingIOError:
                break
        self.notifier.changed(50)
        self.assertEqual(self.notifier.generation, 50)
        self.assertTrue(self.notifier.pending)
        while True:
            try:
                self.read()
            except BlockingIOError:
                break
        self.notifier.consumed(1, 50)
        self.assertEqual(self.read(), (7, 50))

    def test_closed_and_rollback(self):
        self.notifier.changed(3)
        with self.assertRaises(ValueError):
            self.notifier.changed(2)
        self.notifier.close()
        self.notifier.changed(4)
