"""Read-only inherited lifecycle fence, ABI v1; never a peer endpoint.

One aligned native-endian u64 is read through libatomic with acquire ordering.
Immutable source generation and magic bind the owned memfd. No Python byte
slice or mmap decoding substitutes for the final atomic authority read.
"""
import ctypes
import fcntl
import mmap
import os
import struct


class _Buffer(ctypes.Structure):
    _fields_ = [('buf', ctypes.c_void_p), ('obj', ctypes.c_void_p),
                ('length', ctypes.c_ssize_t), ('itemsize', ctypes.c_ssize_t),
                ('readonly', ctypes.c_int), ('ndim', ctypes.c_int),
                ('format', ctypes.c_char_p), ('shape', ctypes.c_void_p),
                ('strides', ctypes.c_void_p), ('suboffsets', ctypes.c_void_p),
                ('internal', ctypes.c_void_p)]


class NativeFence:
    @classmethod
    def inherited(cls, generation):
        value = os.environ.get('RUI_PRIVATE_CONTROL_FD')
        if value is None or not value.isascii() or not value.isdecimal() or len(value) > 10:
            raise ValueError('missing-or-invalid-inherited-native-fence')
        descriptor = int(value)
        try:
            return cls(descriptor, generation)
        finally:
            os.close(descriptor)  # the read-only mapping owns its lifetime

    def __init__(self, descriptor, generation):
        self.mapping = self.buffer = None
        if os.fstat(descriptor).st_size != 4096 or fcntl.fcntl(descriptor, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY:
            raise ValueError('native-fence-size-or-access')
        required = fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_FUTURE_WRITE | fcntl.F_SEAL_SEAL
        if fcntl.fcntl(descriptor, fcntl.F_GET_SEALS) & required != required:
            raise ValueError('native-fence-seals')
        try:
            self.mapping = mmap.mmap(descriptor, 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ)
            if self.mapping[16:24] != b'RUIFENC1' or struct.unpack_from('=Q', self.mapping, 8)[0] != generation:
                raise ValueError('native-fence-incarnation')
            self.atomic = ctypes.CDLL('libatomic.so.1')
            self.load = getattr(self.atomic, '__atomic_load_8')
            self.load.argtypes, self.load.restype = [ctypes.c_void_p, ctypes.c_int], ctypes.c_uint64
            lock_free = getattr(self.atomic, '__atomic_is_lock_free')
            lock_free.argtypes, lock_free.restype = [ctypes.c_size_t, ctypes.c_void_p], ctypes.c_bool
            get_buffer = ctypes.pythonapi.PyObject_GetBuffer
            get_buffer.argtypes, get_buffer.restype = [ctypes.py_object, ctypes.POINTER(_Buffer), ctypes.c_int], ctypes.c_int
            buffer = _Buffer()
            get_buffer(self.mapping, ctypes.byref(buffer), 0)
            self.buffer = buffer
            if not buffer.readonly or buffer.length != 4096 or buffer.buf % 8 or not lock_free(8, buffer.buf):
                raise ValueError('native-fence-atomic-abi')
        except Exception:
            self.close()
            raise

    def current(self):
        if self.buffer is None:
            return 0
        return self.load(self.buffer.buf, 2)  # __ATOMIC_ACQUIRE

    def close(self):
        if self.buffer is not None:
            release = ctypes.pythonapi.PyBuffer_Release
            release.argtypes, release.restype = [ctypes.POINTER(_Buffer)], None
            release(ctypes.byref(self.buffer))
            self.buffer = None
        if self.mapping is not None:
            self.mapping.close()
            self.mapping = None
