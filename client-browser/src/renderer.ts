import type { Timings } from "./timing.js";
import type {
  Affine,
  Color,
  Digest,
  Point,
  Primitive,
  Radii,
  Rect,
  Scene,
  State,
  Stop,
  Viewport,
} from "./model.js";
export const RENDERER_PROFILE = "renderer.browser-canvas-coverage-linear.v1";
const identity: Affine = { xx: 1, yx: 0, xy: 0, yy: 1, tx: 0, ty: 0 };
function multiply(a: Affine, b: Affine): Affine {
  return {
    xx: a.xx * b.xx + a.xy * b.yx,
    xy: a.xx * b.xy + a.xy * b.yy,
    yx: a.yx * b.xx + a.yy * b.yx,
    yy: a.yx * b.xy + a.yy * b.yy,
    tx: a.xx * b.tx + a.xy * b.ty + a.tx,
    ty: a.yx * b.tx + a.yy * b.ty + a.ty,
  };
}
function point(m: Affine, p: Point): Point {
  return {
    x: m.xx * p.x + m.xy * p.y + m.tx,
    y: m.yx * p.x + m.yy * p.y + m.ty,
  };
}
function inverse(m: Affine): Affine {
  const d = m.xx * m.yy - m.xy * m.yx;
  if (!Number.isFinite(d) || d === 0) throw new Error("singular transform");
  return {
    xx: m.yy / d,
    xy: -m.xy / d,
    yx: -m.yx / d,
    yy: m.xx / d,
    tx: (m.xy * m.ty - m.yy * m.tx) / d,
    ty: (m.yx * m.tx - m.xx * m.ty) / d,
  };
}
function transformed(m: Affine, r: Rect): Rect {
  const p = [
    point(m, r),
    point(m, { x: r.x + r.width, y: r.y }),
    point(m, { x: r.x, y: r.y + r.height }),
    point(m, { x: r.x + r.width, y: r.y + r.height }),
  ];
  const x = Math.min(...p.map((v) => v.x)),
    y = Math.min(...p.map((v) => v.y));
  return {
    x,
    y,
    width: Math.max(...p.map((v) => v.x)) - x,
    height: Math.max(...p.map((v) => v.y)) - y,
  };
}
function intersect(a: Rect, b: Rect): Rect {
  const x = Math.max(a.x, b.x),
    y = Math.max(a.y, b.y);
  return {
    x,
    y,
    width: Math.max(0, Math.min(a.x + a.width, b.x + b.width) - x),
    height: Math.max(0, Math.min(a.y + a.height, b.y + b.height) - y),
  };
}
function union(a: Rect, b: Rect): Rect {
  if (!a.width || !a.height) return b;
  if (!b.width || !b.height) return a;
  const x = Math.min(a.x, b.x),
    y = Math.min(a.y, b.y);
  return {
    x,
    y,
    width: Math.max(a.x + a.width, b.x + b.width) - x,
    height: Math.max(a.y + a.height, b.y + b.height) - y,
  };
}
const empty: Rect = { x: 0, y: 0, width: 0, height: 0 };
function integral(r: Rect): Rect {
  return {
    x: Math.floor(r.x),
    y: Math.floor(r.y),
    width: Math.max(0, Math.ceil(r.x + r.width) - Math.floor(r.x)),
    height: Math.max(0, Math.ceil(r.y + r.height) - Math.floor(r.y)),
  };
}
function key(d: Digest): string {
  return d.map((v) => v.toString(16).padStart(2, "0")).join("");
}
interface Path {
  path: Path2D;
  stroke: Path2D;
  bounds: Rect;
  bytes: number;
}
interface Clip {
  matrix: Affine;
  rect: Rect;
}
interface Node {
  id: bigint;
  version: bigint;
  matrix: Affine;
  bounds: Rect;
  clips: Clip[];
  opacity: number;
  primitive: Primitive;
  children: Node[];
}
interface Layer {
  bounds: Rect;
  data: Float32Array;
}
interface Mask {
  bounds: Rect;
  data: Uint8Array;
}
export interface Renderer {
  render(
    state: State,
    viewport: Viewport,
    resources: (digest: Digest) => Uint8Array,
  ): void;
  dispose(): void;
  /** Discard all scope-bound derived state before reset/reconnect/revocation. */
  reset?(): void;
}
/** Canvas provides vector coverage only. All paint interpolation, compositing,
 * isolated opacity and display transfer are explicit premultiplied linear sRGB.
 * Coverage follows the browser engine, not the native Cairo reference profile. */
export class CanvasRenderer implements Renderer {
  private output: CanvasRenderingContext2D;
  private scratch = document.createElement("canvas");
  private context: CanvasRenderingContext2D;
  private paths = new Map<string, Path>();
  private masks = new Map<string, Mask>();
  private cacheBytes = 0;
  private workingBytes = 0;
  private bufferBytes = 0;
  private frame: Float32Array | undefined;
  private image: ImageData | undefined;
  private frameAllocations = 0;
  private outputAllocations = 0;
  private maskBuilds = 0;
  private maskHits = 0;
  private peakBytes = 0;
  private work = 0;
  constructor(
    readonly canvas: HTMLCanvasElement,
    readonly memoryLimit = 64 * 1024 * 1024,
    readonly timings?: Timings,
    readonly optimized = true,
  ) {
    const output = canvas.getContext("2d"),
      context = this.scratch.getContext("2d", { willReadFrequently: true });
    if (!output || !context) throw new Error("Canvas 2D unavailable");
    this.output = output;
    this.context = context;
  }
  private charge(n: number): void {
    const working = this.workingBytes + n;
    if (working + this.cacheBytes + this.bufferBytes > this.memoryLimit) {
      this.paths.clear();
      this.masks.clear();
      this.cacheBytes = 0;
      if (working + this.bufferBytes > this.memoryLimit)
        throw new Error("renderer memory limit");
    }
    this.workingBytes = working;
    this.peakBytes = Math.max(
      this.peakBytes,
      this.workingBytes + this.cacheBytes + this.bufferBytes,
    );
  }
  private path(d: Digest, resources: (digest: Digest) => Uint8Array): Path {
    const k = key(d),
      cached = this.paths.get(k);
    if (cached) return cached;
    const raw = resources(d);
    if (raw.length > 4 * 1024 * 1024) throw new Error("path limit");
    const v = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
    if (raw.length < 8 || v.getUint32(0) !== 0x52565031)
      throw new Error("invalid path");
    const count = v.getUint32(4);
    if (count > 65536) throw new Error("path command limit");
    let offset = 8;
    const path = new Path2D();
    let bounds: Rect = empty;
    const next = (): number => {
      const n = v.getFloat64(offset);
      offset += 8;
      if (!Number.isFinite(n)) throw new Error("invalid path coordinate");
      return n;
    };
    const p = (): [number, number] => {
      const x = next(),
        y = next();
      bounds = unionPoint(bounds, x, y);
      return [x, y];
    };
    const stroke = new Path2D();
    let contour = new Path2D(),
      motion = false,
      last: [number, number] = [0, 0],
      start: [number, number] = [0, 0];
    const differs = (q: [number, number]) =>
      q[0] !== last[0] || q[1] !== last[1];
    const flush = () => {
      if (motion) stroke.addPath(contour);
      contour = new Path2D();
      motion = false;
    };
    for (let i = 0; i < count; i++) {
      switch (v.getUint8(offset++)) {
        case 1: {
          flush();
          const q = p();
          path.moveTo(...q);
          contour.moveTo(...q);
          last = start = q;
          break;
        }
        case 2: {
          const q = p();
          motion ||= differs(q);
          path.lineTo(...q);
          contour.lineTo(...q);
          last = q;
          break;
        }
        case 3: {
          const c = p(),
            q = p();
          motion ||= differs(c) || differs(q);
          path.quadraticCurveTo(...c, ...q);
          contour.quadraticCurveTo(...c, ...q);
          last = q;
          break;
        }
        case 4: {
          const a = p(),
            b = p(),
            q = p();
          motion ||= differs(a) || differs(b) || differs(q);
          path.bezierCurveTo(...a, ...b, ...q);
          contour.bezierCurveTo(...a, ...b, ...q);
          last = q;
          break;
        }
        case 5: {
          motion ||= differs(start);
          path.closePath();
          contour.closePath();
          last = start;
          flush();
          break;
        }
        default:
          throw new Error("invalid path opcode");
      }
    }
    flush();
    if (offset !== raw.length) throw new Error("path trailing bytes");
    const result = { path, stroke, bounds, bytes: raw.length * 4 + 512 };
    this.charge(result.bytes);
    this.workingBytes -= result.bytes;
    this.cacheBytes += result.bytes;
    this.paths.set(k, result);
    return result;
  }
  private geometry(
    p: Primitive,
    resources: (digest: Digest) => Uint8Array,
  ): Rect {
    if (p === "Group") return empty;
    if ("FillPath" in p)
      return this.path(p.FillPath.resource, resources).bounds;
    if ("StrokePath" in p) {
      const s = p.StrokePath,
        b = this.path(s.resource, resources).bounds,
        w = s.width * s.miter_limit;
      return {
        x: b.x - w,
        y: b.y - w,
        width: b.width + 2 * w,
        height: b.height + 2 * w,
      };
    }
    if ("GlyphRun" in p) {
      const g = p.GlyphRun;
      if (!("Outline" in g.font) || g.glyph_ids.some((id) => id !== 0))
        throw new Error("unsupported glyph font");
      const b = this.path(g.font.Outline, resources).bounds;
      let r = empty;
      for (const pos of g.positions)
        r = union(r, {
          x: pos.x + b.x * g.size,
          y: pos.y + b.y * g.size,
          width: b.width * g.size,
          height: b.height * g.size,
        });
      return r;
    }
    return Object.values(p)[0].rect;
  }
  render(
    state: State,
    viewport: Viewport,
    resources: (digest: Digest) => Uint8Array,
  ): void {
    const prepared = this.timings ? performance.now() : 0;
    const { width, height, scale, origin } = viewport;
    if (
      !Number.isInteger(width) ||
      !Number.isInteger(height) ||
      width <= 0 ||
      height <= 0 ||
      width * height > 4 * 1024 * 1024 ||
      scale <= 0 ||
      !Number.isFinite(scale)
    )
      throw new Error("viewport limit");
    this.workingBytes = 0;
    this.work = 0;
    const screen: Rect = { x: 0, y: 0, width, height };
    const matrix: Affine = {
      ...identity,
      xx: scale,
      yy: scale,
      tx: -origin.x * scale,
      ty: -origin.y * scale,
    };
    const session = [...state.records.values()].find((r) => "Session" in r);
    if (!session || !("Session" in session))
      throw new Error("missing session canvas");
    const rootClips: Clip[] = [{ matrix, rect: session.Session.canvas }];
    const compile = (
      scene: Scene,
      parent: Affine,
      clips: Clip[],
      depth: number,
    ): Node => {
      if (depth > 128) throw new Error("scene depth limit");
      const m = multiply(parent, scene.transform),
        c = scene.clip ? [...clips, { matrix: m, rect: scene.clip }] : clips;
      // The compiled graph is independently owned JS memory, beyond pixels.
      this.charge(512 + c.length * 160 + scene.children.length * 32);
      const children = scene.children.map((ref) => {
        const r = state.records.get(ref.id);
        if (!r || !("Scene" in r)) throw new Error("missing scene");
        return compile(r.Scene, m, c, depth + 1);
      });
      let bounds =
        scene.primitive === "Group"
          ? children.reduce((b, n) => union(b, n.bounds), empty)
          : transformed(m, this.geometry(scene.primitive, resources));
      for (const clip of c)
        bounds = intersect(bounds, transformed(clip.matrix, clip.rect));
      bounds = integral(intersect(bounds, screen));
      return {
        id: scene.id,
        version: scene.version,
        matrix: m,
        bounds,
        clips: c,
        opacity: scene.opacity,
        primitive: scene.primitive,
        children,
      };
    };
    const surfaces = [...state.records.values()]
      .flatMap((r) => ("Surface" in r && r.Surface.mapped ? [r.Surface] : []))
      .sort((a, b) =>
        a.stacking_order < b.stacking_order
          ? -1
          : a.stacking_order > b.stacking_order
            ? 1
            : a.id < b.id
              ? -1
              : 1,
      );
    const nodes = surfaces.map((s) => {
      if (s.visual_coverage !== "Complete" || !s.scene_root)
        throw new Error("incomplete visual coverage");
      const m = multiply(
        matrix,
        multiply(
          { ...identity, tx: s.desktop_rect.x, ty: s.desktop_rect.y },
          s.transform,
        ),
      );
      const clips = [
        ...rootClips,
        { matrix: m, rect: { x: 0, y: 0, ...s.content_size } },
        ...(s.clip ? [{ matrix: m, rect: s.clip }] : []),
      ];
      const r = state.records.get(s.scene_root.id);
      if (!r || !("Scene" in r)) throw new Error("missing surface scene");
      const child = compile(r.Scene, m, clips, 0);
      return {
        ...child,
        id: s.id,
        version: s.version,
        primitive: "Group" as Primitive,
        children: [child],
        opacity: s.opacity,
      };
    });
    this.timings?.record(
      "preparation",
      state.revision,
      performance.now() - prepared,
      nodes.length,
    );
    const composed = this.timings ? performance.now() : 0;
    const bytes = width * height * 24;
    let frameData: Float32Array;
    let image: ImageData;
    if (this.optimized) {
      if (
        !this.frame ||
        this.image?.width !== width ||
        this.image?.height !== height
      ) {
        // Retain the old allocation through replacement to account for overlap.
        this.charge(bytes);
        const replacement = new Float32Array(width * height * 4);
        const output = this.output.createImageData(width, height);
        this.frame = replacement;
        this.image = output;
        this.frameAllocations++;
        this.outputAllocations++;
        this.bufferBytes = bytes;
        this.workingBytes -= bytes;
      }
      frameData = this.frame;
      image = this.image!;
      frameData.fill(0);
    } else {
      this.charge(bytes);
      frameData = new Float32Array(width * height * 4);
      image = this.output.createImageData(width, height);
      this.frameAllocations++;
      this.outputAllocations++;
    }
    const frame: Layer = {
      bounds: screen,
      data: frameData,
    };
    for (const node of nodes) this.draw(node, frame, resources);
    this.timings?.record(
      "composition",
      state.revision,
      performance.now() - composed,
      this.work,
    );
    const converted = this.timings ? performance.now() : 0;
    const out = image.data;
    for (let i = 0; i < out.length; i += 4) {
      const a = frame.data[i + 3];
      out[i] = display(a ? frame.data[i] / a : 0);
      out[i + 1] = display(a ? frame.data[i + 1] / a : 0);
      out[i + 2] = display(a ? frame.data[i + 2] / a : 0);
      out[i + 3] = Math.round(Math.max(0, Math.min(1, a)) * 255);
    }
    this.timings?.record(
      "outputConversion",
      state.revision,
      performance.now() - converted,
      width * height,
    );
    const submitted = this.timings ? performance.now() : 0;
    if (this.canvas.width !== width) this.canvas.width = width;
    if (this.canvas.height !== height) this.canvas.height = height;
    this.output.putImageData(image, 0, 0);
    this.timings?.record(
      "submission",
      state.revision,
      performance.now() - submitted,
      width * height,
    );
    this.scratch.width = 0;
    this.scratch.height = 0;
    this.workingBytes = 0;
  }
  private draw(
    node: Node,
    target: Layer,
    resources: (digest: Digest) => Uint8Array,
  ): void {
    const b = node.bounds;
    if (!b.width || !b.height || node.opacity === 0) return;
    const pixels = b.width * b.height;
    this.work += pixels;
    if (this.work > 64 * 1024 * 1024) throw new Error("render work limit");
    if (node.primitive === "Group") {
      if (node.opacity === 1) {
        for (const c of node.children) this.draw(c, target, resources);
        return;
      }
      const bytes = pixels * 16;
      this.charge(bytes);
      const layer: Layer = { bounds: b, data: new Float32Array(pixels * 4) };
      for (const c of node.children) this.draw(c, layer, resources);
      composite(layer, target, node.opacity);
      this.workingBytes -= bytes;
      return;
    }
    const cacheKey = JSON.stringify(
      this.optimized
        ? [node.matrix, node.clips, b, coverageGeometry(node.primitive)]
        : [String(node.id), node.matrix, node.clips, b, node.primitive],
      (_, value) => (typeof value === "bigint" ? String(value) : value),
    );
    let mask = this.masks.get(cacheKey);
    if (!mask) {
      this.maskBuilds++;
      this.charge(pixels * 5);
      this.scratch.width = b.width;
      this.scratch.height = b.height;
      const ctx = this.context;
      for (const c of node.clips) {
        ctx.setTransform(
          c.matrix.xx,
          c.matrix.yx,
          c.matrix.xy,
          c.matrix.yy,
          c.matrix.tx - b.x,
          c.matrix.ty - b.y,
        );
        ctx.beginPath();
        ctx.rect(c.rect.x, c.rect.y, c.rect.width, c.rect.height);
        ctx.clip();
      }
      const m = node.matrix;
      ctx.setTransform(m.xx, m.yx, m.xy, m.yy, m.tx - b.x, m.ty - b.y);
      ctx.fillStyle = "white";
      ctx.strokeStyle = "white";
      this.coverage(ctx, node.primitive, resources);
      const raw = ctx.getImageData(0, 0, b.width, b.height).data,
        data = new Uint8Array(pixels);
      for (let i = 0; i < pixels; i++) data[i] = raw[i * 4 + 3];
      mask = { bounds: b, data };
      this.workingBytes -= pixels * 5;
      const retained = pixels + cacheKey.length * 2 + 256;
      this.charge(retained);
      this.workingBytes -= retained;
      this.cacheBytes += retained;
      this.masks.set(cacheKey, mask);
    } else this.maskHits++;
    const p = node.primitive,
      constant = Object.values(p)[0].color as Color | undefined;
    const inv = constant ? identity : inverse(node.matrix);
    const scratch: Color = { red: 0, green: 0, blue: 0, alpha: 0 };
    for (let y = 0; y < b.height; y++)
      for (let x = 0; x < b.width; x++) {
        const alpha = (mask.data[y * b.width + x] / 255) * node.opacity;
        if (!alpha) continue;
        const c =
          constant ??
          (this.optimized
            ? paintInto(
                p,
                inv.xx * (b.x + x + 0.5) + inv.xy * (b.y + y + 0.5) + inv.tx,
                inv.yx * (b.x + x + 0.5) + inv.yy * (b.y + y + 0.5) + inv.ty,
                scratch,
              )
            : paint(p, point(inv, { x: b.x + x + 0.5, y: b.y + y + 0.5 })));
        const i =
          ((b.y + y - target.bounds.y) * target.bounds.width +
            b.x +
            x -
            target.bounds.x) *
          4;
        const remain = 1 - c.alpha * alpha;
        target.data[i] = c.red * alpha + target.data[i] * remain;
        target.data[i + 1] = c.green * alpha + target.data[i + 1] * remain;
        target.data[i + 2] = c.blue * alpha + target.data[i + 2] * remain;
        target.data[i + 3] = c.alpha * alpha + target.data[i + 3] * remain;
      }
  }
  private coverage(
    ctx: CanvasRenderingContext2D,
    p: Exclude<Primitive, "Group">,
    resources: (digest: Digest) => Uint8Array,
  ): void {
    if ("FillPath" in p) {
      ctx.fill(
        this.path(p.FillPath.resource, resources).path,
        p.FillPath.fill_rule === "EvenOdd" ? "evenodd" : "nonzero",
      );
      return;
    }
    if ("StrokePath" in p) {
      const s = p.StrokePath;
      ctx.lineWidth = s.width;
      ctx.lineCap = s.cap.toLowerCase() as CanvasLineCap;
      ctx.lineJoin = s.join.toLowerCase() as CanvasLineJoin;
      ctx.miterLimit = s.miter_limit;
      ctx.setLineDash(s.dash_lengths);
      ctx.lineDashOffset = s.dash_offset;
      ctx.stroke(this.path(s.resource, resources).stroke);
      return;
    }
    if ("GlyphRun" in p) {
      const g = p.GlyphRun;
      if (!("Outline" in g.font)) throw new Error("missing outline");
      const path = this.path(g.font.Outline, resources).path;
      // Glyph overlaps are a union. Draw into one opaque mask, never blend their paint separately.
      for (const pos of g.positions) {
        ctx.save();
        ctx.translate(pos.x, pos.y);
        ctx.scale(g.size, g.size);
        ctx.fill(path);
        ctx.restore();
      }
      return;
    }
    if ("RoundedRect" in p) {
      const s = p.RoundedRect;
      ctx.fill(rounded(s.rect, s.radii));
      return;
    }
    if ("Border" in p) {
      const s = p.Border;
      if (s.width === 0) return;
      const path = rounded(s.rect, s.radii),
        w = s.width;
      const inner = {
        x: s.rect.x + w,
        y: s.rect.y + w,
        width: s.rect.width - 2 * w,
        height: s.rect.height - 2 * w,
      };
      if (inner.width > 0 && inner.height > 0) {
        const r = Object.fromEntries(
          Object.entries(s.radii).map(([k, v]) => [
            k,
            v.width > w && v.height > w
              ? { width: v.width - w, height: v.height - w }
              : { width: 0, height: 0 },
          ]),
        ) as unknown as Radii;
        path.addPath(rounded(inner, r));
      }
      ctx.fill(path, "evenodd");
      return;
    }
    const r = Object.values(p)[0].rect;
    ctx.fillRect(r.x, r.y, r.width, r.height);
  }
  allocationStats(): {
    ownedBytes: number;
    peakBytes: number;
    frameAllocations: number;
    outputAllocations: number;
    maskBuilds: number;
    maskHits: number;
  } {
    return {
      ownedBytes: this.bufferBytes + this.cacheBytes + this.workingBytes,
      peakBytes: this.peakBytes,
      frameAllocations: this.frameAllocations,
      outputAllocations: this.outputAllocations,
      maskBuilds: this.maskBuilds,
      maskHits: this.maskHits,
    };
  }
  reset(): void {
    this.paths.clear();
    this.masks.clear();
    this.cacheBytes = 0;
    this.frame?.fill(0);
    this.image?.data.fill(0);
    this.frame = undefined;
    this.image = undefined;
    this.bufferBytes = 0;
    this.workingBytes = 0;
    this.scratch.width = 0;
    this.scratch.height = 0;
    this.canvas.width = 0;
    this.canvas.height = 0;
  }
  dispose(): void {
    this.reset();
  }
}
/** Coverage does not depend on paint colors, gradient stops or scalar clusters.
 * Shape, transforms, clips, outline identity and stroke geometry remain keyed. */
function coverageGeometry(p: Exclude<Primitive, "Group">): unknown {
  if ("LinearGradient" in p || "RadialGradient" in p || "SolidRect" in p)
    return { Rect: Object.values(p)[0].rect };
  if ("GlyphRun" in p) {
    const {
      color: _color,
      scalar_clusters: _clusters,
      ...geometry
    } = p.GlyphRun;
    return { GlyphRun: geometry };
  }
  const [kind, fields] = Object.entries(p)[0];
  const { color: _color, ...geometry } = fields;
  return { [kind]: geometry };
}
function unionPoint(b: Rect, x: number, y: number): Rect {
  if (b === empty) return { x, y, width: 0, height: 0 };
  const left = Math.min(b.x, x),
    top = Math.min(b.y, y);
  return {
    x: left,
    y: top,
    width: Math.max(b.x + b.width, x) - left,
    height: Math.max(b.y + b.height, y) - top,
  };
}
function rounded(r: Rect, c: Radii): Path2D {
  const p = new Path2D(),
    x = r.x,
    y = r.y,
    w = r.width,
    h = r.height;
  const tl = c.top_left,
    tr = c.top_right,
    br = c.bottom_right,
    bl = c.bottom_left;
  p.moveTo(x + tl.width, y);
  p.lineTo(x + w - tr.width, y);
  if (tr.width && tr.height)
    p.ellipse(
      x + w - tr.width,
      y + tr.height,
      tr.width,
      tr.height,
      0,
      -Math.PI / 2,
      0,
    );
  else p.lineTo(x + w, y);
  p.lineTo(x + w, y + h - br.height);
  if (br.width && br.height)
    p.ellipse(
      x + w - br.width,
      y + h - br.height,
      br.width,
      br.height,
      0,
      0,
      Math.PI / 2,
    );
  else p.lineTo(x + w, y + h);
  p.lineTo(x + bl.width, y + h);
  if (bl.width && bl.height)
    p.ellipse(
      x + bl.width,
      y + h - bl.height,
      bl.width,
      bl.height,
      0,
      Math.PI / 2,
      Math.PI,
    );
  else p.lineTo(x, y + h);
  p.lineTo(x, y + tl.height);
  if (tl.width && tl.height)
    p.ellipse(
      x + tl.width,
      y + tl.height,
      tl.width,
      tl.height,
      0,
      Math.PI,
      Math.PI * 1.5,
    );
  else p.lineTo(x, y);
  p.closePath();
  return p;
}
function gradient(stops: Stop[], t: number): Color {
  if (t < 0) return stops[0].color;
  if (t > 1) return stops[stops.length - 1].color;
  let left = stops[0];
  for (let i = 1; i < stops.length; i++) {
    const right = stops[i];
    if (t < right.offset) {
      const f = (t - left.offset) / (right.offset - left.offset);
      return {
        red: left.color.red * (1 - f) + right.color.red * f,
        green: left.color.green * (1 - f) + right.color.green * f,
        blue: left.color.blue * (1 - f) + right.color.blue * f,
        alpha: left.color.alpha * (1 - f) + right.color.alpha * f,
      };
    }
    left = right;
  }
  return left.color;
}
function paint(p: Primitive, pos: Point): Color {
  if (p === "Group") throw new Error("group paint");
  if ("LinearGradient" in p) {
    const g = p.LinearGradient,
      dx = g.end.x - g.start.x,
      dy = g.end.y - g.start.y;
    return gradient(
      g.stops,
      ((pos.x - g.start.x) * dx + (pos.y - g.start.y) * dy) /
        (dx * dx + dy * dy),
    );
  }
  if ("RadialGradient" in p) {
    const g = p.RadialGradient;
    return gradient(
      g.stops,
      (Math.hypot(pos.x - g.center.x, pos.y - g.center.y) - g.inner_radius) /
        (g.outer_radius - g.inner_radius),
    );
  }
  return Object.values(p)[0].color;
}
function paintInto(p: Primitive, x: number, y: number, output: Color): Color {
  let stops: Stop[], t: number;
  if (p !== "Group" && "LinearGradient" in p) {
    const g = p.LinearGradient,
      dx = g.end.x - g.start.x,
      dy = g.end.y - g.start.y;
    stops = g.stops;
    t = ((x - g.start.x) * dx + (y - g.start.y) * dy) / (dx * dx + dy * dy);
  } else if (p !== "Group" && "RadialGradient" in p) {
    const g = p.RadialGradient;
    stops = g.stops;
    t =
      (Math.hypot(x - g.center.x, y - g.center.y) - g.inner_radius) /
      (g.outer_radius - g.inner_radius);
  } else throw new Error("non-gradient paint");
  if (t < 0) return stops[0].color;
  if (t > 1) return stops[stops.length - 1].color;
  let low = 1,
    high = stops.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (t < stops[middle].offset) high = middle;
    else low = middle + 1;
  }
  const left = stops[low - 1],
    right = stops[low];
  if (!right) return left.color;
  const f = (t - left.offset) / (right.offset - left.offset);
  output.red = left.color.red * (1 - f) + right.color.red * f;
  output.green = left.color.green * (1 - f) + right.color.green * f;
  output.blue = left.color.blue * (1 - f) + right.color.blue * f;
  output.alpha = left.color.alpha * (1 - f) + right.color.alpha * f;
  return output;
}
function composite(source: Layer, target: Layer, opacity: number): void {
  const b = source.bounds;
  for (let y = 0; y < b.height; y++)
    for (let x = 0; x < b.width; x++) {
      const s = (y * b.width + x) * 4,
        t =
          ((b.y + y - target.bounds.y) * target.bounds.width +
            b.x +
            x -
            target.bounds.x) *
          4,
        a = 1 - source.data[s + 3] * opacity;
      for (let c = 0; c < 4; c++)
        target.data[t + c] =
          source.data[s + c] * opacity + target.data[t + c] * a;
    }
}
// A 16-bit linear lookup has <1 output-byte error, including the dark branch,
// and avoids three transcendental functions per output pixel on every frame.
const transfer = Uint8Array.from({ length: 65536 }, (_, i) => {
  const n = i / 65535;
  return Math.round(
    255 * (n <= 0.0031308 ? 12.92 * n : 1.055 * Math.pow(n, 1 / 2.4) - 0.055),
  );
});
function display(n: number): number {
  return transfer[Math.round(Math.max(0, Math.min(1, n)) * 65535)];
}
