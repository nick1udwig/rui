"""Toolkit-independent, bounded coherent-capture records and exact RUI encoding.

Authentication, projection policy and actual source provenance belong to the
caller. This SDK does not scrape pixels, authenticate, fetch assets or render.
"""
from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
import struct
import uuid


MAX_RECORDS = 20_000
MAX_RESOURCE_BYTES = 16 << 20
MAX_ENVELOPE_BYTES = 1 << 20
MAX_MANIFEST_BYTES = 1 << 20
MAX_TEXT_BYTES = 16 << 10
MAX_COLLECTION_ITEMS = 4096
MAX_MODEL_CACHE_BYTES = 4 << 20


def model_memory(value):
    """Conservative local Python model-cache charge, separate from GTK memory."""
    if isinstance(value, (tuple, list)):
        return 128 + 64 * len(value) + sum(model_memory(item) for item in value)
    if isinstance(value, str):
        return 128 + 4 * len(value.encode('utf-8'))
    return 64


def varint(value: int) -> bytes:
    if value < 0 or value >= 1 << 64:
        raise ValueError("unsigned integer out of range")
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def scalar(tag: int, value: int) -> bytes:
    return varint(tag << 3) + varint(value)


def blob(tag: int, value: bytes) -> bytes:
    return varint(tag << 3 | 2) + varint(len(value)) + value


def text(tag: int, value: str) -> bytes:
    return blob(tag, value.encode("utf-8"))


def doubles(values: list[float]) -> bytes:
    result = bytearray()
    for tag, value in enumerate(values, 1):
        if not math.isfinite(value):
            raise ValueError("nonfinite scalar")
        result.extend(varint(tag << 3 | 1))
        result.extend(struct.pack("<d", 0.0 if value == 0 else value))
    return bytes(result)


def double(tag: int, value: float) -> bytes:
    return varint(tag << 3 | 1) + struct.pack("<d", 0.0 if value == 0 else value)


def ref(kind: int, identifier: int) -> bytes:
    return scalar(1, kind) + scalar(2, identifier)


def path_resource(commands: list[tuple[int, tuple[float, ...]]], glyph=False) -> dict:
    if len(commands) > 100_000:
        raise ValueError("path command ceiling")
    body = bytearray(b"RVP1" + struct.pack(">I", len(commands)))
    for opcode, coordinates in commands:
        if opcode not in (1, 2, 3, 4, 5) or len(coordinates) != {1: 2, 2: 2, 3: 4, 4: 6, 5: 0}[opcode]:
            raise ValueError("invalid path command")
        body.append(opcode)
        for value in coordinates:
            if not math.isfinite(value) or abs(value) > 1e9:
                raise ValueError("path coordinate ceiling")
            body.extend(struct.pack(">d", 0.0 if value == 0 else value))
    if len(body) > MAX_RESOURCE_BYTES:
        raise ValueError("resource byte ceiling")
    kind = provenance = 2 if glyph else 1
    descriptor = struct.pack(">IIQI", kind, 1, len(body), provenance)
    digest = hashlib.sha256(b"RUI/0.1/resource\0" + descriptor + body).hexdigest()
    return {"digest": digest, "kind": kind, "version": 1, "provenance": provenance, "total_bytes": len(body), "body": bytes(body)}


def primitive(value: dict) -> tuple[int, bytes]:
    kind = value["type"]
    if kind == "Group":
        return 10, b""
    if kind == "SolidRect":
        return 11, blob(1, doubles(value["rect"])) + blob(2, doubles(value["color"]))
    if kind == "FillPath":
        return 12, blob(1, bytes.fromhex(value["resource"])) + blob(2, doubles(value["color"])) + scalar(3, value.get("fill_rule", 1))
    if kind == "GlyphRun":
        result = blob(2, bytes.fromhex(value["resource"])) + double(3, value["size"])
        result += blob(4, b"".join(varint(glyph) for glyph in value["glyph_ids"]))
        result += b"".join(blob(5, doubles(position)) for position in value["positions"])
        return 13, result + blob(7, doubles(value["color"]))
    if kind in ("RoundedRect", "Border"):
        radii = b"".join(blob(tag, doubles(radius)) for tag, radius in enumerate(value["radii"], 1))
        result = blob(1, doubles(value["rect"])) + blob(2, radii)
        if kind == "Border":
            return 15, result + double(3, value["width"]) + blob(4, doubles(value["color"]))
        return 14, result + blob(3, doubles(value["color"]))
    if kind == "StrokePath":
        result = blob(1, bytes.fromhex(value["resource"])) + blob(2, doubles(value["color"]))
        result += double(3, value["width"]) + scalar(4, value["cap"]) + scalar(5, value["join"]) + double(6, value["miter_limit"])
        result += blob(7, b"".join(struct.pack("<d", x) for x in value.get("dash_lengths", [])))
        return 18, result + double(8, value.get("dash_offset", 0))
    raise ValueError(f"unsupported SDK primitive: {kind}")


def encode_scene(record: dict) -> bytes:
    data = scalar(1, record["id"]) + scalar(2, record["version"])
    data += b"".join(blob(3, ref(2, child)) for child in record["children"])
    data += blob(4, doubles(record["transform"])) + double(6, record.get("opacity", 1))
    if record.get("clip") is not None:
        data += blob(5, doubles(record["clip"]))
    tag, paint = primitive(record["primitive"])
    return blob(2, data + blob(tag, paint))


def encode_semantic(record: dict) -> bytes:
    data = scalar(1, record["id"]) + scalar(2, record["version"]) + scalar(3, record["role"])
    if "label" in record:
        data += text(4, record["label"])
    if "value" in record:
        data += blob(6, text(1, str(record["value"])))
    data += b"".join(blob(8, ref(3, child)) for child in record["children"])
    data += b"".join(blob(7, scalar(1, key) + scalar(2, value)) for key, value in sorted(record.get("states", {}).items()))
    data += b"".join(blob(10, text(1, action) + scalar(2, 1)) for action in record.get("actions", []))
    if record.get("text_model"):
        data += blob(11, ref(4, record["text_model"]))
    if record.get("collection_model"):
        collection = record["collection_model"]
        model = scalar(1, collection["version"])
        model += b"".join(blob(2, ref(3, item)) for item in collection["items"])
        if collection.get("total_count") is not None:
            model += scalar(3, collection["total_count"])
        model += scalar(4, collection["materialization"])
        model += b"".join(blob(5, ref(3, item)) for item in collection["selected_items"])
        data += blob(12, model)
    # Explicit corpus scope contains only public, complete cooperating semantics.
    data += scalar(16, 1) + scalar(17, 1)
    return blob(3, data)


def encode_text(record: dict) -> bytes:
    data = scalar(1, record["id"]) + scalar(2, record["version"]) + scalar(3, record["text_version"]) + scalar(4, record["status"])
    if record.get("content") is not None:
        data += text(5, record["content"])
    if record.get("selection") is not None:
        data += blob(6, scalar(1, record["selection"][0]) + scalar(2, record["selection"][1]))
    data += scalar(7, record["editable"]) + scalar(8, record["selectable"])
    return blob(4, data)


def encode_session(record: dict) -> bytes:
    data = scalar(1, record["id"]) + scalar(2, record["version"]) + blob(3, doubles(record["canvas"]))
    if record.get("focus"):
        data += blob(5, ref(3, record["focus"])) + blob(6, ref(1, record["focus_surface"]))
    data += scalar(7, record["focus_generation"])
    return blob(6, data)


def encode_surface(frame: dict) -> bytes:
    data = scalar(1, frame["surface_id"]) + scalar(2, frame["revision"]) + scalar(3, frame.get("surface_role", 1))
    data += blob(5, doubles([*frame.get("origin", [0, 0]), *frame["size"]])) + blob(6, doubles(frame["size"]))
    data += blob(7, doubles([1, 0, 0, 1, 0, 0])) + double(9, 1) + scalar(11, frame.get("mapped", True)) + scalar(12, frame.get("focused", False))
    if frame.get("parent_surface"):
        data += blob(4, ref(1, frame["parent_surface"]["surface_id"]))
    if frame.get("scene_root"):
        data += blob(14, ref(2, frame["scene_root"]))
    if frame.get("semantic_root"):
        data += blob(15, ref(3, frame["semantic_root"]))
    routing = frame.get("routing_token")
    if routing is None:
        routing = hashlib.sha256(b"RUI/0.1/route\0" + str(frame["surface_id"]).encode() + frame["capture_token"].encode()).hexdigest()[:32]
    data += blob(17, bytes.fromhex(routing))
    data += scalar(18, 2 if frame["gaps"] else 1) + scalar(19, 1) + scalar(20, 1)
    data += blob(21, frame["capture_token"].encode())
    data += b"".join(text(22, reason) for reason in sorted({gap["reason"] for gap in frame["gaps"]}))
    return blob(1, data)


def encode_snapshot(frame: dict, binding: dict) -> bytes:
    records = [] if frame.get("destroyed") else [encode_surface(frame)]
    if frame.get("parent_surface"):
        records.append(encode_surface({**frame["parent_surface"], "revision": frame["revision"], "capture_token": frame["capture_token"], "gaps": [{"reason": "parent-topology-only"}]}))
    records += [encode_scene(record) for record in frame["scene"]]
    records += [encode_semantic(record) for record in frame["semantics"]]
    records += [encode_text(record["text"]) for record in frame["semantics"] if record.get("text")]
    if frame.get("session"):
        records.append(encode_session(frame["session"]))
    if len(records) > MAX_RECORDS:
        raise ValueError("record ceiling")
    payload = scalar(1, frame["revision"]) + b"".join(blob(2, record) for record in records)
    payload += b"".join(blob(3, bytes.fromhex(digest)) for digest in frame["resources"])
    features = ["content.no-raster.v1", "mirror2d.core.v1", "semantics.core.v1"]
    if frame.get("uses_glyph_outline"):
        features.append("glyph.outline-single.v1")
    payload += b"".join(text(4, name) for name in features)
    envelope = scalar(1, 1)
    envelope += b"".join(blob(tag, bytes.fromhex(binding[key])) for tag, key in [(2, "session_id"), (3, "epoch"), (4, "view_id")])
    envelope += scalar(5, 1) + blob(6, payload)
    envelope += b"".join(text(7, name) for name in features)
    if len(envelope) > MAX_ENVELOPE_BYTES:
        raise ValueError("single-envelope capture ceiling")
    return envelope


class CoherentSource:
    """Stable producer-issued identities and source mutation boundary.

    Reads run synchronously in the source snapshot callback. No independent
    polling, idle callback or accessibility crawl supplies coherent evidence.
    """
    def __init__(self):
        self.identity = uuid.uuid4().hex
        self.source_revision = 1
        self._ids = {}
        self._next_id = 1
        self._semantics = {}
        self._frames = {}
        self.resources = {}
        self._scene_ids = {}
        self._models = {}
        self._model_sizes = {}
        self.model_cache_bytes = 0
        self._focus = {}
        self.retired_scene_ids = set()
        self.retired_semantic_ids = set()
        self.retired_object_ids = set()

    def identifier(self, key: str) -> int:
        # Bounded generated scene paths at depth128 can exceed768 bytes.
        if len(key.encode('utf-8')) > 2048:
            raise ValueError("identity key ceiling")
        if key not in self._ids:
            if len(self._ids) >= MAX_RECORDS:
                raise ValueError("identity ceiling")
            self._ids[key] = self._next_id
            self._next_id += 1
        return self._ids[key]

    def register(self, key: str, role: int, read, *, parent=None, actions=()):
        if key in self._semantics:
            raise ValueError("semantic identity already registered")
        self.identifier("semantic:" + key)
        self._semantics[key] = (role, read, parent, tuple(actions))

    def unregister(self, key: str):
        """Retire a destroyed semantic object and its descendants permanently."""
        removed = {key}
        for _ in range(len(self._semantics)):
            children = {name for name, (_, _, parent, _) in self._semantics.items() if parent in removed}
            if children <= removed:
                break
            removed.update(children)
        for name in removed:
            self._semantics.pop(name, None)
            identifier = self._ids.pop("semantic:" + name, None)
            if identifier is not None:
                self.retired_semantic_ids.add(identifier)
                self.remove_model("collection", identifier)
                self.remove_model("semantic", identifier)
            text_id = self._ids.pop("text:" + name, None)
            if text_id is not None:
                self.retired_semantic_ids.add(text_id)
                self.remove_model("text", text_id)
                self.remove_model("text-record", text_id)
        for surface, (focus, _) in list(self._focus.items()):
            if focus in removed:
                self.focus_changed(surface, None)
        if len(self.retired_semantic_ids) + len(self.retired_scene_ids) + len(self.retired_object_ids) > MAX_RECORDS:
            raise ValueError("retired identity ceiling; start a new epoch")
        self.changed()

    def retire_surface(self, surface):
        """Destroy this source surface and its per-view Session identity.

        Unmapping preserves both identities. Recreating the same registered
        source key starts a fresh surface/view and a fresh Session clock scope.
        """
        keys = ("surface:" + surface, "session:" + surface)
        identifiers = {self._ids[key] for key in keys if key in self._ids}
        if len(self.retired_semantic_ids) + len(self.retired_scene_ids) + len(self.retired_object_ids | identifiers) > MAX_RECORDS:
            raise ValueError("retired identity ceiling; start a new epoch")
        self.retired_object_ids.update(identifiers)
        for key in keys:
            self._ids.pop(key, None)
        self._scene_ids.pop(surface, None)
        self._focus.pop(surface, None)

    def changed(self):
        self.source_revision += 1

    def model_clock(self, kind, identifier, state):
        previous, version = self._models.get((kind, identifier), (state, 1))
        if previous != state:
            version += 1
        key = (kind, identifier)
        charge = model_memory(state) + 256
        next_bytes = self.model_cache_bytes - self._model_sizes.get(key, 0) + charge
        if next_bytes > MAX_MODEL_CACHE_BYTES:
            raise ValueError("source model cache ceiling")
        self.model_cache_bytes = next_bytes
        self._model_sizes[key] = charge
        self._models[(kind, identifier)] = (state, version)
        return version

    def remove_model(self, kind, identifier):
        key = (kind, identifier)
        self._models.pop(key, None)
        self.model_cache_bytes -= self._model_sizes.pop(key, 0)

    def focus_changed(self, surface, focus):
        """Called on actual source focus transitions, including away-and-back."""
        previous, generation = self._focus.get(surface, (None, 1))
        if previous != focus:
            generation += 1
            self._focus[surface] = (focus, generation)
            self.changed()
        else:
            self._focus[surface] = (focus, generation)

    def session(self, surface, revision, size, semantic_ids, *, mapped=True):
        focus, generation = self._focus.get(surface, (None, 1))
        identifier = self._ids.get("semantic:" + focus) if focus else None
        if not mapped or identifier not in semantic_ids:
            identifier = None
        return {"id": self.identifier("session:" + surface), "version": revision, "canvas": [0, 0, *size], "focus": identifier, "focus_surface": self.identifier("surface:" + surface) if identifier else None, "focus_generation": generation}

    def reconcile_scene(self, surface: str, scene: list[dict]):
        current = {record["id"] for record in scene}
        previous = self._scene_ids.get(surface, set())
        disappeared = previous - current
        self.retired_scene_ids.update(disappeared)
        if len(self.retired_scene_ids) + len(self.retired_semantic_ids) + len(self.retired_object_ids) > MAX_RECORDS:
            raise ValueError("retired identity ceiling; start a new epoch")
        for key, identifier in list(self._ids.items()):
            if key.startswith("scene:" + surface + ":") and identifier in disappeared:
                del self._ids[key]
        if current & self.retired_scene_ids:
            raise ValueError("retired scene identity reused")
        self._scene_ids[surface] = current

    def semantics(self, root: str, revision: int) -> list[dict]:
        selected = {root}
        for _ in range(len(self._semantics)):
            added = {key for key, (_, _, parent, _) in self._semantics.items() if parent in selected}
            if added <= selected:
                break
            selected.update(added)
        result = []
        for key, (role, read, parent, actions) in self._semantics.items():
            if key in selected:
                fields = dict(read())
                identifier = self.identifier("semantic:" + key)
                children = [self.identifier("semantic:" + child) for child, (_, _, owner, _) in self._semantics.items() if owner == key and child in selected]
                if fields.get("text"):
                    model = dict(fields["text"])
                    text_id = self.identifier("text:" + key)
                    content = model.get("content")
                    status = model.get("status", 1)
                    if (status == 1) != (content is not None) or (content is not None and len(content.encode("utf-8")) > MAX_TEXT_BYTES):
                        raise ValueError("text content/status ceiling")
                    selection = model.get("selection")
                    if selection is not None and (len(selection) != 2 or any(value < 0 or value > len(content or "") for value in selection)):
                        raise ValueError("scalar selection outside source text")
                    if status not in (1, 2, 3) or (selection is not None and not model.get("selectable")) or (status != 1 and (selection is not None or model.get("editable"))):
                        raise ValueError("text availability/editability mismatch")
                    content_version = self.model_clock("text", text_id, (status, content))
                    # Text record clocks describe the complete actual payload;
                    # content clocks stay independent of selection/editability.
                    record_state = (status, content, content_version,
                                    tuple(selection) if selection is not None else None,
                                    bool(model.get('editable')), bool(model.get('selectable')))
                    model.update(id=text_id, version=self.model_clock("text-record", text_id, record_state), text_version=content_version, status=status)
                    fields["text"], fields["text_model"] = model, text_id
                if fields.get("collection"):
                    model = dict(fields.pop("collection"))
                    items = [self._ids["semantic:" + item] for item in model["items"]]
                    chosen = [self._ids["semantic:" + item] for item in model.get("selected_items", [])]
                    total = model.get("total_count")
                    materialization = model["materialization"]
                    if len(items) > MAX_COLLECTION_ITEMS or len(set(items)) != len(items) or len(set(chosen)) != len(chosen) or not set(chosen) <= set(items) or not set(items) <= set(children):
                        raise ValueError("collection membership ceiling")
                    if materialization not in (1, 2) or (total is not None and total < len(items)) or (materialization == 1 and total != len(items)):
                        raise ValueError("collection materialization/count mismatch")
                    model.update(items=items, selected_items=chosen, version=self.model_clock("collection", identifier, (tuple(items), total, materialization, tuple(chosen))))
                    fields["collection_model"] = model
                node = {"id": identifier, "identity": key, "role": role, "children": children, "actions": list(actions), **fields}
                # This is the actual SemanticNode payload, not the overall
                # visual frame. Text content/selection have their own record
                # and clocks, so only the TextModel reference participates.
                stamp = json.dumps({name: value for name, value in node.items() if name not in ('identity', 'text')}, sort_keys=True, separators=(',', ':'))
                node['version'] = self.model_clock('semantic', identifier, stamp)
                result.append(node)
        return result

    def write(self, output: Path, frame: dict):
        output.mkdir(parents=True, exist_ok=True)
        binding = {"session_id": "01" * 16, "epoch": self.identity, "view_id": f"{frame['surface_id']:032x}"}
        label = f"{frame['surface_key']}-{frame['revision']:04d}"
        frame["routing_token"] = hashlib.sha256(b"RUI/0.1/route\0" + str(frame["surface_id"]).encode() + frame["capture_token"].encode()).hexdigest()[:32]
        envelope = encode_snapshot(frame, binding)
        resources = []
        for digest in frame["resources"]:
            resource = self.resources[digest]
            if resource["kind"] not in (1, 2) or resource["provenance"] != resource["kind"] or not resource["body"].startswith(b"RVP1"):
                raise ValueError("SDK geometry-only resource handoff")
            (output / (digest + ".rvp")).write_bytes(resource["body"])
            resources.append({key: value for key, value in resource.items() if key != "body"})
        frame["binding"] = binding
        frame["envelope_sha256"] = hashlib.sha256(envelope).hexdigest()
        if frame.get("exporter_counters", {}).get("schema") == 2:
            frame["protocol_handoff"] = {"envelope_bytes": len(envelope),
                "resource_bytes": sum(item["total_bytes"] for item in resources),
                "resource_kind_counts": {str(kind): sum(item["kind"] == kind for item in resources) for kind in (1, 2)}}
        summary = {**frame, "resource_offers": resources}
        metadata = json.dumps(summary, indent=2, sort_keys=True) + "\n"
        if len(metadata.encode()) > MAX_MANIFEST_BYTES:
            raise ValueError('capture manifest ceiling')
        (output / (label + ".bin")).write_bytes(envelope)
        (output / (label + ".json")).write_text(metadata)
        # The artifact is the consumer handoff. No source memory cache or hidden
        # in-flight history retains old bodies after the exact files are written.
        self.resources.clear()
        return label
