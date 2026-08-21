// The shared live view of the array: WebGL2 point cloud fed by the WS preview
// stream. Optionally interactive: "tap" fires a callback with the polar position
// (View tab → burst), "draw" streams stroke dabs to the backend (Draw tab).

import { useEffect, useRef, useState } from "react";
import { useGate } from "./state";
import type { PenKind, PreviewFrame, PreviewMeta } from "./types";

const VS = `#version 300 es
layout(location=0) in vec2 a_pos;
layout(location=1) in vec3 a_color;
uniform float u_point_size;
out vec3 v_color;
void main() {
  v_color = a_color;
  gl_Position = vec4(a_pos, 0.0, 1.0);
  gl_PointSize = u_point_size;
}`;

const FS = `#version 300 es
precision mediump float;
in vec3 v_color;
out vec4 frag;
void main() {
  vec2 d = gl_PointCoord - 0.5;
  float radius = length(d);
  float core = 1.0 - smoothstep(0.22, 0.42, radius);
  float halo = (1.0 - smoothstep(0.38, 0.5, radius)) * 0.32;
  float a = max(core, halo);
  // Premultiplied output; alpha accumulates so bright light occludes the page.
  frag = vec4(v_color * a, a);
}`;

interface Gl {
  gl: WebGL2RenderingContext;
  program: WebGLProgram;
  posBuf: WebGLBuffer;
  colorBuf: WebGLBuffer;
  count: number;
  pointSizeLoc: WebGLUniformLocation;
}

function buildGl(
  canvas: HTMLCanvasElement,
  meta: PreviewMeta,
  pixel0AtInner = false,
): Gl | null {
  // Transparent canvas: the array's light composites additively over the page,
  // so there is no background rectangle at all — the page gradient shows through
  // around and inside the ring.
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true });
  if (!gl) return null;

  const prog = gl.createProgram();
  for (const [type, src] of [
    [gl.VERTEX_SHADER, VS],
    [gl.FRAGMENT_SHADER, FS],
  ] as const) {
    const sh = gl.createShader(type)!;
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      console.error("stage preview shader failed", gl.getShaderInfoLog(sh));
      gl.deleteShader(sh);
      gl.deleteProgram(prog);
      return null;
    }
    gl.attachShader(prog, sh);
  }
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    console.error("stage preview program failed", gl.getProgramInfoLog(prog));
    gl.deleteProgram(prog);
    return null;
  }
  gl.useProgram(prog);

  const { spokes, pixels } = meta;
  const inner = meta.inner_radius_ft / meta.outer_radius_ft;
  const count = spokes * pixels;
  const positions = new Float32Array(count * 2);
  for (let s = 0; s < spokes; s++) {
    const theta = (s / spokes) * Math.PI * 2 - Math.PI / 2; // spoke 0 at top
    for (let i = 0; i < pixels; i++) {
      const t = pixels > 1 ? i / (pixels - 1) : 0;
      const radial = pixel0AtInner ? t : 1 - t;
      const r = (inner + radial * (1 - inner)) * 0.95;
      const o = (s * pixels + i) * 2;
      positions[o] = r * Math.cos(theta);
      positions[o + 1] = r * Math.sin(theta);
    }
  }
  const posBuf = gl.createBuffer()!;
  gl.bindBuffer(gl.ARRAY_BUFFER, posBuf);
  gl.bufferData(gl.ARRAY_BUFFER, positions, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

  const colorBuf = gl.createBuffer()!;
  gl.bindBuffer(gl.ARRAY_BUFFER, colorBuf);
  gl.bufferData(gl.ARRAY_BUFFER, count * 3, gl.DYNAMIC_DRAW);
  gl.enableVertexAttribArray(1);
  gl.vertexAttribPointer(1, 3, gl.UNSIGNED_BYTE, true, 0, 0);

  gl.clearColor(0, 0, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE);

  return {
    gl,
    program: prog,
    posBuf,
    colorBuf,
    count,
    pointSizeLoc: gl.getUniformLocation(prog, "u_point_size")!,
  };
}

/** An alternate source for the stage preview (for example an archived show recording). */
export interface GatePreviewSource {
  meta: PreviewMeta;
  /** Uprising recordings store each spoke from the inner ring outward. */
  pixel0AtInner?: boolean;
  subscribe: (listener: (frame: PreviewFrame) => void) => () => void;
}

export interface DrawPen {
  pen: PenKind;
  hue: number; // turns; -1 = white
  saturation: number;
  brightness: number;
  size: number;
  intensity: number;
}

export default function GateCanvas({
  onTap,
  drawPen,
  previewSource,
}: {
  /** Called with (angle, radius01) on a click/tap (when not drawing). */
  onTap?: (angle: number, radius: number) => void;
  /** When set, pointer drags stream Paint messages with this pen. */
  drawPen?: DrawPen;
  /** Replaces the live backend stream. Used by the archived-show replay screen. */
  previewSource?: GatePreviewSource;
}) {
  const { client } = useGate();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const glRef = useRef<Gl | null>(null);
  const [meta, setMeta] = useState<PreviewMeta | null>(previewSource?.meta ?? null);
  const pending = useRef<{ angle: number; radius: number; dir: number }[]>([]);
  // Per-pointer stroke state (multitouch: each finger tracked separately).
  // Last cartesian position feeds the motion direction for directional pens.
  const pointers = useRef(new Map<number, { px: number; py: number; dir: number }>());
  const penRef = useRef(drawPen);
  penRef.current = drawPen;

  // Subscribe to the preview stream while mounted (resubscribe on reconnect).
  // Phones use a moderate downsample: enough to protect venue WiFi while keeping
  // the radial lines crisp on high-density mobile displays.
  useEffect(() => {
    if (previewSource) {
      setMeta(previewSource.meta);
      return;
    }
    const phone = window.innerWidth < 700;
    const decimate = phone ? 3 : 1;
    const sub = () => client.subscribePreview(phone ? 24 : 30, decimate);
    sub();
    const offStatus = client.onStatus((up) => up && sub());
    const offMsg = client.onMessage((m) => {
      if (m.type === "preview_meta") setMeta(m);
    });
    return () => {
      offStatus();
      offMsg();
      client.send({ type: "unsubscribe_preview" });
    };
  }, [client, previewSource]);

  useEffect(() => {
    if (!meta || !canvasRef.current) return;
    glRef.current = buildGl(canvasRef.current, meta, previewSource?.pixel0AtInner);
    return () => {
      // Release resources, but do not deliberately lose the canvas context:
      // React StrictMode performs a setup-cleanup-setup cycle in development and
      // a lost context cannot be synchronously reused on the second setup.
      const current = glRef.current;
      if (current) {
        current.gl.deleteBuffer(current.posBuf);
        current.gl.deleteBuffer(current.colorBuf);
        current.gl.deleteProgram(current.program);
      }
      glRef.current = null;
    };
  }, [meta, previewSource]);

  useEffect(() => {
    const draw = (frame: PreviewFrame) => {
      const g = glRef.current;
      const canvas = canvasRef.current;
      if (!g || !canvas) return;
      const { gl } = g;
      const size = Math.min(canvas.clientWidth, canvas.clientHeight);
      const dpr = Math.min(window.devicePixelRatio || 1, 3);
      const backingSize = Math.max(1, Math.round(size * dpr));
      if (canvas.width !== backingSize) {
        canvas.width = backingSize;
        canvas.height = backingSize;
        gl.viewport(0, 0, canvas.width, canvas.height);
      }
      const n = Math.min(g.count, frame.spokes * frame.pixels);
      gl.bindBuffer(gl.ARRAY_BUFFER, g.colorBuf);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, frame.rgb.subarray(0, n * 3));
      // Points must overlap along a spoke (spacing ≈ 0.45·radius/pixels) or the
      // array reads as dim dotted lines instead of continuous light.
      gl.uniform1f(g.pointSizeLoc, Math.max(2.5, (canvas.width / frame.pixels) * 0.82));
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.drawArrays(gl.POINTS, 0, n);
    };
    return previewSource ? previewSource.subscribe(draw) : client.onFrame(draw);
  }, [client, previewSource]);

  // Flush accumulated stroke points ~30x/s.
  useEffect(() => {
    const interval = setInterval(() => {
      const pen = penRef.current;
      if (!pen || pending.current.length === 0) return;
      client.send({
        type: "paint",
        pen: pen.pen,
        points: pending.current,
        hue: pen.hue,
        saturation: pen.saturation,
        brightness: pen.brightness,
        size: pen.size,
        intensity: pen.intensity,
      });
      pending.current = [];
    }, 33);
    return () => clearInterval(interval);
  }, [client]);

  const toPolar = (e: { clientX: number; clientY: number }) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
    const y = -(((e.clientY - rect.top) / rect.height) * 2 - 1);
    return {
      angle: Math.atan2(y, x) + Math.PI / 2, // undo spoke-0-at-top rotation
      radius: Math.min(1.2, Math.hypot(x, y) / 0.95),
    };
  };

  // Explicit tool modes — no gesture guessing. Tap mode (onTap set): every press
  // fires immediately on pointer-DOWN (snappy for tapping on the beat) and drags
  // never draw. Draw mode (drawPen set): the press paints from the first contact;
  // taps are just one-dab strokes. Per pointer, so multitouch works in both.
  const cart = (polar: { angle: number; radius: number }) => ({
    x: polar.radius * Math.cos(polar.angle),
    y: polar.radius * Math.sin(polar.angle),
  });

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (onTap) {
      const p = toPolar(e);
      onTap(p.angle, Math.min(1, p.radius));
      return;
    }
    if (!drawPen) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const polar = toPolar(e);
    const c = cart(polar);
    pointers.current.set(e.pointerId, { px: c.x, py: c.y, dir: 0 });
    pending.current.push({ ...polar, dir: 0 });
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!drawPen) return;
    const p = pointers.current.get(e.pointerId);
    if (!p) return;
    const native = e.nativeEvent as PointerEvent;
    const events = native.getCoalescedEvents?.() ?? [native];
    for (const ev of events) {
      const polar = toPolar(ev);
      const c = cart(polar);
      const dx = c.x - p.px;
      const dy = c.y - p.py;
      if (Math.hypot(dx, dy) > 0.004) {
        p.dir = Math.atan2(dy, dx);
        p.px = c.x;
        p.py = c.y;
      }
      pending.current.push({ ...polar, dir: p.dir });
    }
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    pointers.current.delete(e.pointerId);
  };

  return (
    <canvas
      ref={canvasRef}
      className="gate-canvas"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={(e) => pointers.current.delete(e.pointerId)}
      style={{ touchAction: "none", cursor: drawPen ? "crosshair" : onTap ? "pointer" : "default" }}
    />
  );
}
