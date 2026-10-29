"""Bounded writes for explicitly owned capture artifacts."""


def write_artifact(path, body, *, reuse=False):
    if reuse:
        try:
            with path.open('rb') as existing:
                view = memoryview(body)
                if all(existing.read(65536) == view[offset:offset + 65536] for offset in range(0, len(body), 65536)) and not existing.read(1):
                    return
        except FileNotFoundError:
            pass
    path.write_bytes(body)
