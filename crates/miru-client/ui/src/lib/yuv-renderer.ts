/**
 * WebGL2 YUV → RGB renderer.
 *
 * ⚠️ FROZEN — not wired into the live session. See ADR 0013.
 *   This renderer assumed the bottleneck was per-frame JPEG decode in JS.
 *   The real bottleneck is IPC bandwidth: feeding it raw I420 over the
 *   Tauri (JSON) bridge would cost ~124 MB/s at 1080p30 vs ~8 MB/s for the
 *   current JPEG-over-IPC path — a ~15x regression. The correct way to drop
 *   the double-codec is WebCodecs VideoDecoder (decode VP9 in the WebView),
 *   at which point this file should be deleted. Kept only as reference.
 *
 * Why it was written: JPEG decoding per frame in JavaScript is ~30ms at 1080p.
 *      WebGL2 with Y/U/V as luminance textures + fragment shader = ~1ms.
 *
 * Pipeline:
 *   1. Allocate three GL_R8 textures (Y, U, V) sized to frame dimensions
 *   2. On each frame: glTexSubImage2D the planes
 *   3. Fragment shader: BT.601 YUV→RGB conversion
 */

const VERT_SRC = `#version 300 es
in vec2 a_pos;
out vec2 v_uv;
void main() {
  v_uv = vec2(a_pos.x * 0.5 + 0.5, 1.0 - (a_pos.y * 0.5 + 0.5));
  gl_Position = vec4(a_pos, 0.0, 1.0);
}`;

const FRAG_SRC = `#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_y;
uniform sampler2D u_u;
uniform sampler2D u_v;
out vec4 fragColor;

void main() {
  // BT.601 limited-range to RGB
  float y = texture(u_y, v_uv).r;
  float u = texture(u_u, v_uv).r - 0.5;
  float v = texture(u_v, v_uv).r - 0.5;

  // Limited range expansion: y ∈ [16/255, 235/255]
  y = (y - 16.0/255.0) * (255.0/219.0);
  u = u * (255.0/224.0);
  v = v * (255.0/224.0);

  vec3 rgb = vec3(
    y + 1.402  * v,
    y - 0.344 * u - 0.714 * v,
    y + 1.772 * u
  );
  fragColor = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}`;

export class YuvRenderer {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private vao: WebGLVertexArrayObject;
  private texY: WebGLTexture;
  private texU: WebGLTexture;
  private texV: WebGLTexture;
  private width = 0;
  private height = 0;

  constructor(canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", {
      antialias: false,
      preserveDrawingBuffer: false,
      premultipliedAlpha: false,
    });
    if (!gl) throw new Error("WebGL2 not supported");
    this.gl = gl;

    this.program = compileProgram(gl, VERT_SRC, FRAG_SRC);

    // Fullscreen triangle
    this.vao = gl.createVertexArray()!;
    gl.bindVertexArray(this.vao);
    const vbo = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 3, -1, -1, 3]),
      gl.STATIC_DRAW,
    );
    const posLoc = gl.getAttribLocation(this.program, "a_pos");
    gl.enableVertexAttribArray(posLoc);
    gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);

    this.texY = createPlaneTex(gl, 0);
    this.texU = createPlaneTex(gl, 1);
    this.texV = createPlaneTex(gl, 2);

    gl.useProgram(this.program);
    gl.uniform1i(gl.getUniformLocation(this.program, "u_y"), 0);
    gl.uniform1i(gl.getUniformLocation(this.program, "u_u"), 1);
    gl.uniform1i(gl.getUniformLocation(this.program, "u_v"), 2);
  }

  /** Upload I420 planes and render. */
  render(y: Uint8Array, u: Uint8Array, v: Uint8Array, w: number, h: number) {
    const gl = this.gl;
    if (w !== this.width || h !== this.height) {
      this.resize(w, h);
    }

    uploadPlane(gl, this.texY, 0, y, w, h);
    uploadPlane(gl, this.texU, 1, u, w >> 1, h >> 1);
    uploadPlane(gl, this.texV, 2, v, w >> 1, h >> 1);

    gl.viewport(0, 0, gl.canvas.width, gl.canvas.height);
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  private resize(w: number, h: number) {
    const gl = this.gl;
    this.width = w;
    this.height = h;
    (gl.canvas as HTMLCanvasElement).width = w;
    (gl.canvas as HTMLCanvasElement).height = h;

    // Pre-allocate textures with their final size
    allocPlane(gl, this.texY, 0, w, h);
    allocPlane(gl, this.texU, 1, w >> 1, h >> 1);
    allocPlane(gl, this.texV, 2, w >> 1, h >> 1);
  }

  destroy() {
    const gl = this.gl;
    gl.deleteProgram(this.program);
    gl.deleteVertexArray(this.vao);
    gl.deleteTexture(this.texY);
    gl.deleteTexture(this.texU);
    gl.deleteTexture(this.texV);
  }
}

// ─── helpers ──────────────────────────────────────────────────────────────────

function compileShader(
  gl: WebGL2RenderingContext,
  type: number,
  src: string,
): WebGLShader {
  const sh = gl.createShader(type)!;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(sh);
    gl.deleteShader(sh);
    throw new Error(`shader compile: ${log}`);
  }
  return sh;
}

function compileProgram(
  gl: WebGL2RenderingContext,
  vertSrc: string,
  fragSrc: string,
): WebGLProgram {
  const v = compileShader(gl, gl.VERTEX_SHADER, vertSrc);
  const f = compileShader(gl, gl.FRAGMENT_SHADER, fragSrc);
  const p = gl.createProgram()!;
  gl.attachShader(p, v);
  gl.attachShader(p, f);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(p);
    gl.deleteProgram(p);
    throw new Error(`program link: ${log}`);
  }
  return p;
}

function createPlaneTex(gl: WebGL2RenderingContext, unit: number): WebGLTexture {
  gl.activeTexture(gl.TEXTURE0 + unit);
  const tex = gl.createTexture()!;
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return tex;
}

function allocPlane(
  gl: WebGL2RenderingContext,
  tex: WebGLTexture,
  unit: number,
  w: number,
  h: number,
) {
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, w, h, 0, gl.RED, gl.UNSIGNED_BYTE, null);
}

function uploadPlane(
  gl: WebGL2RenderingContext,
  tex: WebGLTexture,
  unit: number,
  data: Uint8Array,
  w: number,
  h: number,
) {
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RED, gl.UNSIGNED_BYTE, data);
}
