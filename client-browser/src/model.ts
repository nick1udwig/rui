/** Read-only copies of the Rust committed model. Wire identities never use Number. */
export type Id = bigint;
export type Digest = number[];
export interface Point {
  x: number;
  y: number;
}
export interface Rect extends Point {
  width: number;
  height: number;
}
export interface Size {
  width: number;
  height: number;
}
export interface Affine {
  xx: number;
  yx: number;
  xy: number;
  yy: number;
  tx: number;
  ty: number;
}
export interface Color {
  red: number;
  green: number;
  blue: number;
  alpha: number;
}
export interface Radii {
  top_left: Size;
  top_right: Size;
  bottom_right: Size;
  bottom_left: Size;
}
export interface Ref {
  kind: "Surface" | "Scene" | "Semantic" | "Text" | "Binding" | "Session";
  id: Id;
}
export interface Stop {
  offset: number;
  color: Color;
}
export type Primitive =
  | "Group"
  | { SolidRect: { rect: Rect; color: Color } }
  | { RoundedRect: { rect: Rect; radii: Radii; color: Color } }
  | { Border: { rect: Rect; radii: Radii; width: number; color: Color } }
  | {
      FillPath: {
        resource: Digest;
        color: Color;
        fill_rule: "Nonzero" | "EvenOdd";
      };
    }
  | {
      StrokePath: {
        resource: Digest;
        color: Color;
        width: number;
        cap: "Butt" | "Round" | "Square";
        join: "Miter" | "Round" | "Bevel";
        miter_limit: number;
        dash_lengths: number[];
        dash_offset: number;
      };
    }
  | {
      GlyphRun: {
        font: { Outline: Digest } | { Fingerprint: Digest };
        size: number;
        glyph_ids: number[];
        positions: Point[];
        scalar_clusters: bigint[];
        color: Color;
      };
    }
  | {
      LinearGradient: {
        rect: Rect;
        start: Point;
        end: Point;
        stops: Stop[];
        spread: "Pad";
      };
    }
  | {
      RadialGradient: {
        rect: Rect;
        center: Point;
        inner_radius: number;
        outer_radius: number;
        stops: Stop[];
        spread: "Pad";
      };
    };
export interface Scene {
  id: Id;
  version: bigint;
  children: Ref[];
  transform: Affine;
  clip: Rect | null;
  opacity: number;
  primitive: Primitive;
  resources: Digest[];
}
export interface Surface {
  id: Id;
  version: bigint;
  role: "Toplevel" | "Popup" | "Tooltip" | "Shell";
  parent: Ref | null;
  desktop_rect: Rect;
  content_size: Size;
  transform: Affine;
  clip: Rect | null;
  opacity: number;
  stacking_order: bigint;
  mapped: boolean;
  focused: boolean;
  modal_for: Ref | null;
  semantic_root: Ref | null;
  input_region: Rect[];
  routing_token: number[];
  semantic_coverage: "Rich" | "Partial" | "Opaque";
  capture_consistency: "Coherent" | "Observed";
  source_capture_tokens: number[][];
  unsupported_features: string[];
  scene_root: Ref | null;
  visual_coverage: "Complete" | "Partial" | "Unsupported";
}
export type TriState = "False" | "True" | "Unknown";
export type TypedValue =
  | { Text: string }
  | { Boolean: boolean }
  | { Number: number }
  | { Integer: bigint };
export interface Action {
  id: string;
  schema_version: number;
}
export interface Relation {
  kind: "LabelFor" | "LabelledBy" | "Controls" | "Describes" | "MemberOf";
  targets: Ref[];
}
export interface Collection {
  version: bigint;
  items: Ref[];
  total_count: bigint | null;
  materialization: "Full" | "Partial";
  selected_items: Ref[];
}
export interface Semantic {
  browser_input_type?: string;
  id: Id;
  version: bigint;
  role:
    | "Root"
    | "Group"
    | "Button"
    | "Checkbox"
    | "TextField"
    | "Label"
    | "List"
    | "ListItem"
    | "CustomCanvas";
  label: string | null;
  description: string | null;
  value: TypedValue | null;
  actions: Action[];
  children: Ref[];
  relations: Relation[];
  text: Ref | null;
  collection: Collection | null;
  bindings: Ref[];
  semantic_group: Ref | null;
  reading_order: bigint | null;
  sensitivity: "Public" | "Sensitive" | "Redacted";
  completeness: "Full" | "Incomplete" | "Opaque";
  states: ReadonlyMap<
    "Disabled" | "Checked" | "Selected" | "Expanded" | "ReadOnly",
    TriState
  >;
}
export interface Text {
  id: Id;
  version: bigint;
  text_version: bigint;
  content: string | null;
  status: "Available" | "Unavailable" | "Redacted";
  selection: { anchor: bigint; focus: bigint } | null;
  editable: boolean;
  selectable: boolean;
}
export interface Cursor {
  position: Point;
  hotspot: Point;
  appearance:
    { Shape: "Default" | "Text" | "Pointer" | "Hidden" } | { Outline: Digest };
}
export interface SessionState {
  id: Id;
  version: bigint;
  cursor: Cursor | null;
  canvas: Rect;
  focus: Ref | null;
  focus_surface: Ref | null;
  focus_generation: bigint;
}
export interface Binding {
  id: Id;
  version: bigint;
  scenes: Ref[];
  surface: Ref;
  semantics: Ref[];
  interaction_region: Rect | null;
}
export type Record =
  | { Scene: Scene }
  | { Surface: Surface }
  | { Semantic: Semantic }
  | { Text: Text }
  | { Session: SessionState }
  | { Binding: Binding };
export interface State {
  revision: bigint;
  records: ReadonlyMap<Id, Record>;
  retired: readonly Id[];
}
export interface Viewport {
  width: number;
  height: number;
  origin: Point;
  scale: number;
}
export type SessionEvent =
  | { type: "connected"; can_control: boolean }
  | { type: "viewOpened"; width: number; height: number }
  | { type: "applied" | "resourcesNeeded"; revision: bigint }
  | { type: "control"; granted: boolean }
  | {
      type: "commandResult";
      sequence: bigint;
      status: string;
      rejection: string;
      source_commit: string;
      revision: bigint | null;
    }
  | { type: "error"; code: string }
  | { type: "none" | "resumed" | "reset" };
export function semantic(state: State, target: Id): Semantic | undefined {
  const r = state.records.get(target);
  return r && "Semantic" in r ? r.Semantic : undefined;
}
export function text(state: State, target: Id): Text | undefined {
  const n = semantic(state, target);
  const r = n?.text && state.records.get(n.text.id);
  return r && "Text" in r ? r.Text : undefined;
}
/** DOM uses UTF-16; the protocol uses Unicode scalar offsets. Reject split surrogates. */
export function scalarOffset(value: string, utf16: number): bigint {
  if (
    !Number.isInteger(utf16) ||
    utf16 < 0 ||
    utf16 > value.length ||
    (utf16 > 0 &&
      utf16 < value.length &&
      /[\uD800-\uDBFF]/.test(value[utf16 - 1]) &&
      /[\uDC00-\uDFFF]/.test(value[utf16]))
  )
    throw new Error("invalid UTF-16 boundary");
  return BigInt([...value.slice(0, utf16)].length);
}

export function assertUint64(value: bigint): void {
  if (typeof value !== "bigint" || value < 0n || value > 0xffffffffffffffffn)
    throw new RangeError("expected an unsigned 64-bit integer");
}
