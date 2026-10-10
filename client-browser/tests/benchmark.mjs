import { CanvasRenderer } from "../dist/renderer.js";
import { Timings } from "../dist/timing.js";

const matrix = { xx: 1, yx: 0, xy: 0, yy: 1, tx: 0, ty: 0 };
const rect = { x: 0, y: 0, width: 256, height: 256 };
const color = (i) => ({
  red: (i % 5) / 5,
  green: (i % 7) / 7,
  blue: (i % 11) / 11,
  alpha: 0.75,
});
function state(step) {
  const scene = (id, primitive, children = [], opacity = 1, clip = null) => ({
    Scene: {
      id: BigInt(id),
      version: BigInt(step + 1),
      primitive,
      children: children.map((id) => ({ kind: "Scene", id: BigInt(id) })),
      opacity,
      clip,
      transform: matrix,
      resources: [],
    },
  });
  const shift = Math.floor(step / 20) % 3;
  const area = { x: 12 + shift, y: 12, width: 230 - shift, height: 230 };
  const corners = Object.fromEntries(
    ["top_left", "top_right", "bottom_right", "bottom_left"].map((k) => [
      k,
      { width: 20, height: 14 },
    ]),
  );
  const records = [
    {
      Session: {
        id: 1n,
        version: 1n,
        canvas: rect,
        focus: null,
        focus_generation: 1n,
      },
    },
    {
      Surface: {
        id: 2n,
        version: 1n,
        desktop_rect: rect,
        content_size: { width: 256, height: 256 },
        transform: matrix,
        clip: null,
        opacity: 1,
        stacking_order: 0n,
        mapped: true,
        scene_root: { kind: "Scene", id: 3n },
        visual_coverage: "Complete",
      },
    },
    scene(3, "Group", [4, 5, 6]),
    scene(4, { SolidRect: { rect, color: color(step) } }),
    scene(5, "Group", [7, 8], 0.6, { x: 8, y: 8, width: 240, height: 240 }),
    scene(7, {
      RoundedRect: { rect: area, radii: corners, color: color(step + 1) },
    }),
    scene(8, {
      Border: { rect: area, radii: corners, width: 3, color: color(step + 2) },
    }),
    scene(6, {
      LinearGradient: {
        rect: { x: 50, y: 50, width: 120, height: 120 },
        start: { x: 50, y: 50 },
        end: { x: 170, y: 170 },
        spread: "Pad",
        stops: [
          { offset: 0, color: color(step + 3) },
          { offset: 0.5, color: color(step + 4) },
          { offset: 1, color: color(step + 5) },
        ],
      },
    }),
  ];
  return {
    revision: BigInt(step + 1),
    records: new Map(records.map((r) => [Object.values(r)[0].id, r])),
    retired: [],
  };
}
const viewport = { width: 256, height: 256, origin: { x: 0, y: 0 }, scale: 1 };
const resources = () => {
  throw new Error("unexpected resource");
};

export function measure(timed = true) {
  const canvas = document.createElement("canvas");
  document.body.append(canvas);
  const recorder = new Timings(4096);
  const renderer = new CanvasRenderer(canvas, 8 << 20, recorder);
  // Companion exact full-render comparisons are excluded from timed samples.
  for (let i = 0; i < 25; i++) {
    const current = state(i * 9);
    renderer.render(current, viewport, resources);
    const output = canvas.getContext("2d").getImageData(0, 0, 256, 256).data;
    const freshCanvas = document.createElement("canvas");
    const fresh = new CanvasRenderer(freshCanvas, 8 << 20, undefined, false);
    fresh.render(current, viewport, resources);
    const oracle = freshCanvas
      .getContext("2d")
      .getImageData(0, 0, 256, 256).data;
    if (output.some((v, j) => v !== oracle[j]))
      throw new Error("cached/full renderer mismatch");
    fresh.dispose();
  }
  if (!timed) {
    renderer.dispose();
    canvas.remove();
    return { exactReferenceFrames: 25 };
  }
  for (let i = 0; i < 20; i++) renderer.render(state(i), viewport, resources);
  recorder.clear();
  const samples = [];
  for (let i = 0; i < 600; i++) {
    const current = state(i);
    const start = performance.now();
    renderer.render(current, viewport, resources);
    samples.push(performance.now() - start);
  }
  const timing = recorder.report();
  const ownership = renderer.allocationStats?.() ?? null;
  renderer.dispose();
  canvas.remove();
  const disposed = renderer.allocationStats?.() ?? null;
  return {
    frames: 600,
    oracleFrames: 25,
    viewport,
    recordCount: 8,
    conditions:
      "CPU/canvas warm draw only; 20 warmups; exact fresh-render companion excluded; no transport/presentation claim",
    samples,
    timing,
    ownership,
    disposed,
  };
}
