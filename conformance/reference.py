#!/usr/bin/env python3
"""Independent, dependency-free decoder for the golden RUI fixture subset.

This is a test oracle, not a production RUI client. Its supported subset is
explicit: unsupported fixture records raise errors instead of being ignored.
"""

from __future__ import annotations

import struct
import copy
import hashlib

MAX_BYTES = 16 * 1024 * 1024


def varint(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 70, 7):
        if offset >= len(data):
            raise ValueError("truncated varint")
        byte = data[offset]
        offset += 1
        if shift == 63 and byte > 1:
            raise ValueError("overflowing varint")
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
    raise ValueError("overflowing varint")


def fields(data: bytes) -> dict[int, list[tuple[int, object]]]:
    """Parse protobuf wire values without generated bindings or Rust code."""
    if len(data) > MAX_BYTES:
        raise ValueError("fixture too large")
    result: dict[int, list[tuple[int, object]]] = {}
    offset = 0
    while offset < len(data):
        key, offset = varint(data, offset)
        tag, wire = key >> 3, key & 7
        if not 0 < tag < (1 << 29):
            raise ValueError("invalid tag")
        if wire == 0:
            value, offset = varint(data, offset)
        elif wire in (1, 5):
            size = 8 if wire == 1 else 4
            end = offset + size
            if end > len(data):
                raise ValueError("truncated fixed value")
            value = data[offset:end]
            offset = end
        elif wire == 2:
            size, offset = varint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("truncated bytes")
            value = data[offset:end]
            offset = end
        else:
            raise ValueError("unsupported protobuf wire type")
        result.setdefault(tag, []).append((wire, value))
    return result


def one(message, tag, wire, default=None):
    values = message.get(tag, [])
    if not values:
        return default
    if any(actual != wire for actual, _ in values):
        raise ValueError("wrong wire type")
    return values[-1][1]


def many(message, tag, wire):
    values = message.get(tag, [])
    if any(actual != wire for actual, _ in values):
        raise ValueError("wrong wire type")
    return [value for _, value in values]


def number(message, tag, default=0):
    return one(message, tag, 0, default)


def string(message, tag, default=None):
    raw = one(message, tag, 2)
    return default if raw is None else raw.decode("utf-8", errors="strict")


def double(message, tag, default=0.0):
    raw = one(message, tag, 1)
    return default if raw is None else struct.unpack("<d", raw)[0]


def packed_uints(message, tag):
    values = []
    for wire, data in message.get(tag, []):
        if wire == 0:
            values.append(data)
        elif wire == 2:
            offset = 0
            while offset < len(data):
                value, offset = varint(data, offset)
                values.append(value)
        else:
            raise ValueError("wrong packed wire type")
    return values


def reference_ref(data):
    item = fields(data)
    if number(item, 1) != 3 or number(item, 2) == 0:
        raise ValueError("expected a semantic reference")
    return number(item, 2)


def semantic_record(data):
    wrapper = fields(data)
    raw = one(wrapper, 3, 2)
    if raw is None or any(tag in wrapper for tag in (1, 2, 4, 5, 6)):
        raise ValueError("oracle supports semantic-only fixtures")
    record = fields(raw)
    if any(tag in record for tag in (6, 7, 9, 10, 11, 12, 13, 14, 15)):
        raise ValueError("unsupported semantic fixture field")
    if number(record, 3) not in range(1, 10):
        raise ValueError("invalid role")
    if number(record, 16) != 1 or number(record, 17) != 1:
        raise ValueError("fixture must be public and complete")
    result = {
        "id": number(record, 1),
        "version": number(record, 2),
        "role": [None, "Root", "Group", "Button", "Checkbox", "TextField",
                 "Label", "List", "ListItem", "CustomCanvas"][number(record, 3)],
        "sensitivity": "Public",
        "completeness": "Full",
        "label": string(record, 4),
        "description": string(record, 5),
        "children": [reference_ref(ref) for ref in many(record, 8, 2)],
    }
    if result["id"] == 0 or result["version"] == 0:
        raise ValueError("zero identity or version")
    return result


class ReferenceReplica:
    def __init__(self):
        self.revision = 0
        self.records = {}
        self.retired = set()
        self.history = {}

    def state(self):
        return {
            "revision": self.revision,
            "retired": sorted(self.retired),
            "records": [self.records[key] for key in sorted(self.records)],
        }

    def apply(self, kind, payload):
        candidate = copy.deepcopy(self)
        candidate._apply(kind, payload)
        self.__dict__ = candidate.__dict__

    def _apply(self, kind, payload):
        publication = fields(payload)
        if kind == "snapshot":
            if self.revision or number(publication, 1) == 0:
                raise ValueError("oracle only accepts initial snapshot")
            if publication.get(3) or publication.get(4):
                raise ValueError("unsupported fixture resource or feature")
            self.revision = number(publication, 1)
            for raw in many(publication, 2, 2):
                record = semantic_record(raw)
                if record["id"] in self.records:
                    raise ValueError("duplicate id")
                self.records[record["id"]] = record
        elif kind == "transaction":
            base, new = number(publication, 1), number(publication, 2)
            digest = hashlib.sha256(payload).digest()
            if new in self.history:
                if self.history[new] != digest:
                    raise ValueError("conflicting duplicate")
                return
            if base != self.revision or new != base + 1:
                raise ValueError("invalid revision")
            if publication.get(4) or publication.get(5):
                raise ValueError("unsupported fixture resource or feature")
            touched = set()
            for raw in many(publication, 3, 2):
                mutation = fields(raw)
                put, delete = one(mutation, 1, 2), one(mutation, 2, 2)
                if (put is None) == (delete is None):
                    raise ValueError("unknown or conflicting mutation")
                if put is not None:
                    record = semantic_record(put)
                    object_id = record["id"]
                    previous = self.records.get(object_id)
                    version = 1 if previous is None else previous["version"] + 1
                    if object_id in self.retired or record["version"] != version:
                        raise ValueError("version or retired identity")
                    self.records[object_id] = record
                else:
                    object_id = reference_ref(delete)
                    if object_id not in self.records:
                        raise ValueError("unknown deletion")
                    del self.records[object_id]
                    self.retired.add(object_id)
                if object_id in touched:
                    raise ValueError("duplicate mutation")
                touched.add(object_id)
            self.revision = new
            self.history[new] = digest
        else:
            raise ValueError("unknown publication kind")
        owner = {}
        for record in self.records.values():
            for child in record["children"]:
                if child not in self.records or child in owner:
                    raise ValueError("dangling child or multiple owners")
                owner[child] = record["id"]
        for object_id in self.records:
            seen = set()
            while object_id in owner:
                if object_id in seen or len(seen) >= 128:
                    raise ValueError("cycle or depth")
                seen.add(object_id)
                object_id = owner[object_id]
