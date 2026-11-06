"""Bounded trace decompression through libzstd, independent of the Rust decoder."""
import ctypes as c
from ctypes.util import find_library
import hashlib

MAX_CHUNK = 64 << 10
HISTORY_BYTES = 256 << 10


def library():
    lib = c.CDLL(find_library('zstd') or 'libzstd.so.1')
    signatures = {
        'ZSTD_getFrameContentSize': (c.c_ulonglong, [c.c_void_p, c.c_size_t]),
        'ZSTD_findFrameCompressedSize': (c.c_size_t, [c.c_void_p, c.c_size_t]),
        'ZSTD_getDictID_fromFrame': (c.c_uint, [c.c_void_p, c.c_size_t]),
        'ZSTD_createDCtx': (c.c_void_p, []),
        'ZSTD_freeDCtx': (c.c_size_t, [c.c_void_p]),
        'ZSTD_decompress_usingDict': (c.c_size_t, [c.c_void_p, c.c_void_p, c.c_size_t, c.c_void_p, c.c_size_t, c.c_void_p, c.c_size_t]),
        'ZSTD_isError': (c.c_uint, [c.c_size_t]),
    }
    for name, (result, args) in signatures.items():
        function = getattr(lib, name)
        function.restype, function.argtypes = result, args
    return lib


class ChunkHistory:
    def __init__(self):
        self.history = b''
        self.lib = None

    def clear(self):
        self.history = b''

    def decode(self, kind, data):
        if kind not in (12, 15, 16):
            raise ValueError('unknown publication chunk kind')
        if kind != 12:
            dictionary = b''
            if kind == 16:
                if len(data) < 32 or data[:32] != hashlib.sha256(self.history).digest():
                    raise ValueError('publication compression history mismatch')
                data, dictionary = data[32:], self.history
            if self.lib is None:
                self.lib = library()
            lib = self.lib
            size = lib.ZSTD_getFrameContentSize(data, len(data))
            if (not 0 < size <= MAX_CHUNK
                    or lib.ZSTD_findFrameCompressedSize(data, len(data)) != len(data)
                    or lib.ZSTD_getDictID_fromFrame(data, len(data)) != 0):
                raise ValueError('compressed publication frame bounds')
            if data[4] & 32:
                window = size
            else:
                base = 1 << (10 + (data[5] >> 3))
                window = base + (base >> 3) * (data[5] & 7)
            if window > (HISTORY_BYTES if kind == 16 else MAX_CHUNK):
                raise ValueError('compressed publication window bounds')
            output = c.create_string_buffer(size)
            context = lib.ZSTD_createDCtx()
            if not context:
                raise ValueError('cannot allocate trace decompressor')
            try:
                count = lib.ZSTD_decompress_usingDict(context, output, size, data, len(data), dictionary, len(dictionary))
                if lib.ZSTD_isError(count) or count != size:
                    raise ValueError('invalid compressed publication chunk')
                data = output.raw
            finally:
                lib.ZSTD_freeDCtx(context)
        if not data or len(data) > MAX_CHUNK:
            raise ValueError('publication chunk bounds')
        self.history = (self.history + data)[-HISTORY_BYTES:]
        return data
