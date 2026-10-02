#!/usr/bin/env python3
"""Run exact golden bytes and seeded replay against two independent reducers."""

import json
from pathlib import Path
import random
import subprocess
from shrink import shrink_transcript

from reference import ReferenceReplica

ROOT = Path(__file__).resolve().parent.parent


def vint(value):
    encoded = bytearray()
    while value > 127:
        encoded.append((value & 127) | 128)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def uint(tag, value):
    return vint(tag << 3) + vint(value)


def blob(tag, value):
    return vint((tag << 3) | 2) + vint(len(value)) + value


def ref(object_id):
    return uint(1, 3) + uint(2, object_id)


def record(object_id, version, label=None, description=None, children=()):
    value = uint(1, object_id) + uint(2, version) + uint(3, 2)
    if label is not None:
        value += blob(4, label.encode())
    if description is not None:
        value += blob(5, description.encode())
    value += b"".join(blob(8, ref(child)) for child in children)
    value += uint(16, 1) + uint(17, 1)
    return blob(3, value)


def snapshot(revision, records):
    return uint(1, revision) + b"".join(blob(2, item) for item in records)


def transaction(base, puts=(), deletes=()):
    return (
        uint(1, base)
        + uint(2, base + 1)
        + b"".join(blob(3, blob(1, item)) for item in puts)
        + b"".join(blob(3, blob(2, ref(item))) for item in deletes)
    )


def entry(kind, payload):
    return {"kind": kind, "hex": payload.hex()}


def golden():
    first = snapshot(1, [record(1, 1, "Controls", children=[2, 3]),
                         record(2, 1, "Save", "Cleared on replacement"),
                         record(3, 1, "A😀e\u0301אב")])
    change = transaction(1, [record(2, 2, "Saved")])
    yield entry("snapshot", first)
    yield entry("transaction", change)
    yield entry("transaction", change)  # identical retry
    yield entry("transaction", transaction(2, [record(2, 4, "bad version")]))
    yield entry("transaction", transaction(2, deletes=[2]))  # dangling edge
    yield entry("transaction", transaction(2, [record(1, 2, "Controls", children=[3, 2])]))
    yield entry("transaction", transaction(3, [record(1, 3, "Controls", children=[3])], [2]))
    yield entry("transaction", transaction(4, [record(2, 1, "reused")]))
    yield entry("transaction", transaction(4, [record(3, 2, "cycle", children=[1])]))
    yield entry("transaction", uint(1, 4) + uint(2, 5) + blob(3, blob(99, b"")))
    yield entry("transaction", transaction(4, [record(3, 2, "Final")]))


def seeded(seed):
    rng = random.Random(seed)
    versions = {object_id: 1 for object_id in range(1, 18)}
    children = list(range(2, 18))
    yield entry("snapshot", snapshot(1, [record(1, 1, "root", children=children)] +
                                     [record(i, 1, f"item {i}") for i in children]))
    revision = 1
    for step in range(64):
        object_id = rng.choice(list(versions))
        versions[object_id] += 1
        if object_id == 1:
            rng.shuffle(children)
        value = record(object_id, versions[object_id], f"seed {seed} step {step} 😀",
                       children=children if object_id == 1 else ())
        payload = transaction(revision, [value])
        yield entry("transaction", payload)
        if step % 9 == 0:
            yield entry("transaction", payload)
        if step % 13 == 0:
            yield entry("transaction", transaction(revision + 8, [value]))
        revision += 1


class ReplayMismatch(AssertionError):
    def __init__(self, category, detail):
        self.category = category
        super().__init__(detail)


def compare(entries):
    oracle = ReferenceReplica()
    expected = []
    for item in entries:
        try:
            oracle.apply(item["kind"], bytes.fromhex(item["hex"]))
            accepted = True
        except ValueError:
            accepted = False
        expected.append({"accepted": accepted, "state": oracle.state()})
    result = subprocess.run(
        [str(ROOT / "target/debug/examples/replay")],
        input="".join(json.dumps(item) + "\n" for item in entries),
        text=True, capture_output=True, check=True, cwd=ROOT,
    )
    actual = [json.loads(line) for line in result.stdout.splitlines()]
    if len(actual) != len(expected):
        raise ReplayMismatch('response-count', f"response count: {len(actual)} != {len(expected)}")
    for index, (left, right) in enumerate(zip(actual, expected)):
        if left != right:
            category = 'admission' if left['accepted'] != right['accepted'] else 'state'
            raise ReplayMismatch(category, f"prefix {index}: Rust={left!r}\nPython={right!r}")
    return len(expected)


def verify(entries):
    try:
        return compare(entries)
    except ReplayMismatch as original:
        def reproduces(candidate):
            try:
                compare(candidate)
            except ReplayMismatch as mismatch:
                return mismatch.category == original.category
            return False

        minimized, runs = shrink_transcript(entries, reproduces)
        destination = ROOT / 'target/conformance-failure.jsonl'
        destination.write_text(''.join(json.dumps(item) + '\n' for item in minimized))
        raise AssertionError(f'{original}\nReduced {len(entries)} operations to {len(minimized)} '
                             f'in {runs} runs; exact reproducer: {destination}') from original


def main():
    subprocess.run(["cargo", "build", "--offline", "--locked", "-p", "rui-protocol-core", "--examples"],
                   cwd=ROOT, check=True)
    path = ROOT / "conformance/golden/semantic.jsonl"
    stored = [json.loads(line) for line in path.read_text().splitlines()]
    if stored != list(golden()):
        raise AssertionError("golden bytes changed; review the schema/fixture deliberately")
    checked = verify(stored)
    for seed in range(16):
        entries = list(seeded(seed))
        checked += verify(entries)
        oracle = ReferenceReplica()
        for item in entries:
            try:
                oracle.apply(item["kind"], bytes.fromhex(item["hex"]))
            except ValueError:
                pass
        direct = snapshot(oracle.revision, [
            record(node["id"], node["version"], node["label"],
                   node["description"], node["children"])
            for node in oracle.state()["records"]
        ])
        checked += verify([entry("snapshot", direct)])
    print(f"Independent replay passed: {checked} prefixes, 16 seeds, exact golden bytes")
    from scene_reference import verify_scenes
    verify_scenes()


if __name__ == "__main__":
    main()
