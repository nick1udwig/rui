import { CanvasRenderer } from "../dist/renderer.js";
import { scalarOffset } from "../dist/model.js";
function assert(ok, message) {
  if (!ok) throw new Error(message);
}
const I = { xx: 1, yx: 0, xy: 0, yy: 1, tx: 0, ty: 0 },
  rect = { x: 0, y: 0, width: 32, height: 32 };
const red = { red: 1, green: 0, blue: 0, alpha: 1 },
  green = { red: 0, green: 1, blue: 0, alpha: 1 },
  blue = { red: 0, green: 0, blue: 1, alpha: 1 },
  white = { red: 1, green: 1, blue: 1, alpha: 1 };
const solid = (color, area = rect) => ({ SolidRect: { rect: area, color } });
const scene = (
  id,
  primitive,
  children = [],
  opacity = 1,
  transform = I,
  clip = null,
) => ({
  Scene: {
    id: BigInt(id),
    version: 1n,
    primitive,
    children: children.map((id) => ({ kind: "Scene", id: BigInt(id) })),
    opacity,
    transform,
    clip,
    resources: [],
  },
});
function state(records) {
  const all = [
    { Session: { id: 1n, canvas: rect, focus: null, focus_generation: 0n } },
    {
      Surface: {
        id: 2n,
        version: 1n,
        desktop_rect: rect,
        content_size: { width: 32, height: 32 },
        transform: I,
        clip: null,
        opacity: 1,
        stacking_order: 0n,
        mapped: true,
        scene_root: { kind: "Scene", id: 3n },
        visual_coverage: "Complete",
      },
    },
    ...records,
  ];
  return {
    revision: 1n,
    records: new Map(all.map((r) => [Object.values(r)[0].id, r])),
    retired: [],
  };
}
function pixel(canvas, x, y) {
  return [...canvas.getContext("2d").getImageData(x, y, 1, 1).data];
}
function close(actual, expected, message) {
  assert(
    actual.every((v, i) => Math.abs(v - expected[i]) <= 1),
    `${message}: ${actual} != ${expected}`,
  );
}
function rvp(
  ops = [[1, 0, 0], [2, 8, 0], [3, 10, 4, 8, 8], [4, 6, 10, 2, 10, 0, 8], [5]],
) {
  const bytes = new Uint8Array(
      8 + ops.reduce((n, o) => n + 1 + (o.length - 1) * 8, 0),
    ),
    v = new DataView(bytes.buffer);
  v.setUint32(0, 0x52565031);
  v.setUint32(4, ops.length);
  let at = 8;
  for (const o of ops) {
    bytes[at++] = o[0];
    for (const n of o.slice(1)) {
      v.setFloat64(at, n);
      at += 8;
    }
  }
  return bytes;
}
export function checkPixels() {
  const canvas = document.createElement("canvas");
  document.body.append(canvas);
  let renderer = new CanvasRenderer(canvas);
  const render = (records, resource = () => rvp()) => {
    renderer.dispose();
    renderer = new CanvasRenderer(canvas);
    renderer.render(
      state(records),
      { width: 32, height: 32, origin: { x: 0, y: 0 }, scale: 1 },
      resource,
    );
  };
  render([
    scene(3, "Group", [4, 5]),
    scene(4, solid(green)),
    scene(5, solid({ red: 0.5, green: 0, blue: 0, alpha: 0.5 })),
  ]);
  close(pixel(canvas, 10, 10), [188, 188, 0, 255], "linear source-over");
  render([
    scene(3, "Group", [4, 5]),
    scene(4, solid(white)),
    scene(5, "Group", [6, 7], 0.5),
    scene(6, solid(red)),
    scene(7, solid(blue, { x: 8, y: 8, width: 16, height: 16 })),
  ]);
  close(pixel(canvas, 4, 4), [255, 188, 188, 255], "isolated group outside");
  close(pixel(canvas, 12, 12), [188, 188, 255, 255], "isolated group overlap");
  render([
    scene(3, {
      LinearGradient: {
        rect,
        start: { x: 0, y: 0 },
        end: { x: 31, y: 0 },
        stops: [
          { offset: 0, color: red },
          { offset: 1, color: blue },
        ],
        spread: "Pad",
      },
    }),
  ]);
  close(pixel(canvas, 15, 15), [188, 0, 188, 255], "linear gradient midpoint");
  render([
    scene(3, {
      LinearGradient: {
        rect,
        start: { x: 0, y: 0 },
        end: { x: 1, y: 0 },
        stops: [
          { offset: 0, color: red },
          { offset: 0.5, color: green },
          { offset: 0.5, color: blue },
          { offset: 1, color: blue },
        ],
        spread: "Pad",
      },
    }),
  ]);
  close(
    pixel(canvas, 0, 1),
    [0, 0, 255, 255],
    "duplicate gradient stops select last",
  );
  render([
    scene(3, {
      RadialGradient: {
        rect,
        center: { x: 15.5, y: 15.5 },
        inner_radius: 0,
        outer_radius: 16,
        stops: [
          { offset: 0, color: red },
          { offset: 1, color: blue },
        ],
        spread: "Pad",
      },
    }),
  ]);
  close(pixel(canvas, 15, 15), [255, 0, 0, 255], "radial center");
  render([
    scene(3, "Group", [4], 1, I, { x: 4, y: 4, width: 8, height: 8 }),
    scene(4, solid(red), [], 1, { ...I, tx: 2, ty: 2 }),
  ]);
  close(pixel(canvas, 2, 6), [0, 0, 0, 0], "ancestor clip");
  close(pixel(canvas, 6, 6), [255, 0, 0, 255], "transformed content in clip");
  const radii = {
    top_left: { width: 8, height: 8 },
    top_right: { width: 8, height: 8 },
    bottom_left: { width: 8, height: 8 },
    bottom_right: { width: 8, height: 8 },
  };
  render([scene(3, { RoundedRect: { rect, radii, color: red } })]);
  close(pixel(canvas, 0, 0), [0, 0, 0, 0], "rounded corner");
  close(pixel(canvas, 15, 15), [255, 0, 0, 255], "rounded interior");
  render([scene(3, { Border: { rect, radii, width: 2, color: red } })]);
  close(pixel(canvas, 15, 15), [0, 0, 0, 0], "inward border interior");
  close(pixel(canvas, 15, 0), [255, 0, 0, 255], "inward border edge");
  const digest = Array(32).fill(1);
  render([
    scene(3, {
      FillPath: { resource: digest, color: green, fill_rule: "Nonzero" },
    }),
  ]);
  close(pixel(canvas, 4, 4), [0, 255, 0, 255], "RVP quadratic/cubic fill");
  render([
    scene(3, {
      StrokePath: {
        resource: digest,
        color: blue,
        width: 2,
        cap: "Butt",
        join: "Miter",
        miter_limit: 4,
        dash_lengths: [],
        dash_offset: 0,
      },
    }),
  ]);
  close(pixel(canvas, 4, 0), [0, 0, 255, 255], "RVP stroke");
  render([
    scene(3, {
      GlyphRun: {
        font: { Outline: digest },
        size: 2,
        glyph_ids: [0],
        positions: [{ x: 3, y: 3 }],
        scalar_clusters: [0n],
        color: red,
      },
    }),
  ]);
  close(
    pixel(canvas, 11, 11),
    [255, 0, 0, 255],
    "source outline glyph size/position",
  );
  renderer.render(
    state([scene(3, solid(blue))]),
    { width: 32, height: 32, origin: { x: 0, y: 0 }, scale: 1 },
    () => rvp(),
  );
  close(
    pixel(canvas, 11, 11),
    [0, 0, 255, 255],
    "same IDs after session replacement cannot reuse stale masks",
  );
  render(
    [
      scene(3, {
        StrokePath: {
          resource: digest,
          color: blue,
          width: 4,
          cap: "Round",
          join: "Round",
          miter_limit: 1,
          dash_lengths: [],
          dash_offset: 0,
        },
      }),
    ],
    () => rvp([[1, 4, 4], [2, 4, 4], [5]]),
  );
  close(pixel(canvas, 4, 4), [0, 0, 0, 0], "zero-length round stroke is empty");
  render(
    [
      scene(3, {
        StrokePath: {
          resource: digest,
          color: blue,
          width: 2,
          cap: "Butt",
          join: "Miter",
          miter_limit: 1,
          dash_lengths: [4, 4],
          dash_offset: 2,
        },
      }),
    ],
    () =>
      rvp([
        [1, 0, 4],
        [2, 24, 4],
      ]),
  );
  close(
    pixel(canvas, 1, 4),
    [0, 0, 255, 255],
    "positive dash phase starts inside the first dash",
  );
  close(pixel(canvas, 3, 4), [0, 0, 0, 0], "dash gap");
  render([scene(3, { Border: { rect, radii, width: 0, color: red } })]);
  close(pixel(canvas, 0, 8), [0, 0, 0, 0], "zero-width border is empty");
  assert(scalarOffset("A😀éאב", 3) === 2n, "UTF-16 to Unicode scalars");
  let rejected = false;
  try {
    scalarOffset("A😀", 2);
  } catch {
    rejected = true;
  }
  assert(rejected, "split surrogate accepted");
  let limited = false;
  try {
    renderer.render(
      state([scene(3, solid(red))]),
      { width: 4096, height: 4096, origin: { x: 0, y: 0 }, scale: 1 },
      () => rvp(),
    );
  } catch {
    limited = true;
  }
  assert(limited, "pixel ceiling ignored");
  renderer.dispose();
  assert(
    canvas.width === 0 && canvas.height === 0,
    "canvas retained after disposal",
  );
  canvas.remove();
  return {
    primitives: 9,
    linearBlending: true,
    isolatedOpacity: true,
    clips: true,
    sourceGlyphs: true,
    unicodeScalars: true,
    limitsAndDisposal: true,
  };
}
