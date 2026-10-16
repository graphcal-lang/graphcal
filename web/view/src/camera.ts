// The canvas is a free sheet: the bars stay put, the stage pans and zooms.
// The arithmetic is pure (and tested); `Camera` puts it on the stage and
// owns the zoom controls: the mode buttons, the percent and its popover.

export interface CameraState {
  x: number;
  y: number;
  scale: number;
}

export const MIN_SCALE = 0.1;
export const MAX_SCALE = 4;

export function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/** Zoom to `scale` (clamped), keeping the canvas point (`px`, `py`) where it is. */
export function zoomAround(
  camera: CameraState,
  px: number,
  py: number,
  scale: number,
): CameraState {
  const next = clampScale(scale);
  return {
    x: px - ((px - camera.x) * next) / camera.scale,
    y: py - ((py - camera.y) * next) / camera.scale,
    scale: next,
  };
}

export interface Size {
  width: number;
  height: number;
}

/** The stage scaled down (never up) to fit the canvas, centred. */
export function overviewOf(canvas: Size, content: Size): CameraState {
  const scale = Math.min(1, canvas.width / content.width, canvas.height / content.height);
  return {
    scale,
    x: Math.max(0, (canvas.width - content.width * scale) / 2),
    y: Math.max(0, (canvas.height - content.height * scale) / 2),
  };
}

/** At full size: a stage larger than the canvas shows its middle, not its left edge. */
export function centredOf(canvas: Size, content: Size): CameraState {
  return {
    scale: 1,
    x: content.width > canvas.width ? Math.round((canvas.width - content.width) / 2) : 0,
    y: content.height > canvas.height ? Math.round((canvas.height - content.height) / 2) : 0,
  };
}

/**
 * fit: text at full size; low-activity intermediate nodes are folded, one by
 * one, until the sheet fits the canvas, or until none are left to fold (then
 * it overflows and is panned). overview: everything open, the whole stage
 * scaled down to fit.
 */
export type Mode = "fit" | "overview";

export function isMode(value: string | undefined): value is Mode {
  return value === "fit" || value === "overview";
}

export interface CameraElements {
  canvas: HTMLElement;
  stage: HTMLElement;
  zoom: HTMLButtonElement;
  popover: HTMLElement;
  number: HTMLInputElement;
  modes: readonly HTMLButtonElement[];
}

export class Camera {
  private state: CameraState = { x: 0, y: 0, scale: 1 };
  /** Whether the camera was moved by hand: no mode is active any more, the percent is. */
  manual = false;
  private readonly els: CameraElements;
  private drag: { x: number; y: number } | null = null;

  constructor(els: CameraElements) {
    this.els = els;
    this.listen();
  }

  get scale(): number {
    return this.state.scale;
  }

  apply(): void {
    const { stage, zoom, number } = this.els;
    const { x, y, scale } = this.state;
    stage.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
    const percent = String(Math.round(scale * 100));
    zoom.textContent = `${percent}%`;
    if (document.activeElement !== number) number.value = percent;
  }

  /** Measuring needs an unscaled stage; `apply` puts the camera back. */
  unscale(): void {
    this.els.stage.style.transform = "";
  }

  set(state: CameraState): void {
    this.state = state;
    this.apply();
  }

  reset(): void {
    this.set({ x: 0, y: 0, scale: 1 });
  }

  /** A mode was applied: its button is active, the percent is not. */
  showMode(mode: Mode): void {
    this.manual = false;
    for (const b of this.els.modes) b.classList.toggle("active", b.dataset.mode === mode);
    this.els.zoom.classList.remove("active");
  }

  private moved(state: CameraState): void {
    this.set(state);
    this.manual = true;
    for (const b of this.els.modes) b.classList.remove("active");
    this.els.zoom.classList.add("active");
  }

  /** Zoom to an absolute scale, keeping the point under the canvas centre fixed. */
  zoomTo(scale: number): void {
    const { canvas } = this.els;
    this.moved(zoomAround(this.state, canvas.clientWidth / 2, canvas.clientHeight / 2, scale));
  }

  /** Close the zoom popover. */
  closePopover(): void {
    this.els.popover.hidden = true;
  }

  private listen(): void {
    const { canvas, zoom, popover, number } = this.els;
    // Wheel: pan; with ctrl / cmd (or a trackpad pinch): zoom around the cursor.
    canvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        if (event.ctrlKey || event.metaKey) {
          const rect = canvas.getBoundingClientRect();
          const scale = this.state.scale * Math.exp(-event.deltaY * 0.01);
          this.moved(
            zoomAround(this.state, event.clientX - rect.left, event.clientY - rect.top, scale),
          );
        } else {
          const { x, y, scale } = this.state;
          this.moved({ x: x - event.deltaX, y: y - event.deltaY, scale });
        }
      },
      { passive: false },
    );
    // Drag on empty canvas: pan.
    canvas.addEventListener("pointerdown", (event) => {
      // Pointer capture would retarget the click to the canvas, so boxes, ruler
      // labels and pane headers never start a pan.
      const target = event.target as HTMLElement;
      if (event.button !== 0 || target.closest(".col, .ruler-inner, .pane-head")) return;
      this.drag = { x: event.clientX, y: event.clientY };
      canvas.setPointerCapture(event.pointerId);
      canvas.classList.add("panning");
    });
    canvas.addEventListener("pointermove", (event) => {
      if (!this.drag) return;
      const { x, y, scale } = this.state;
      const dx = event.clientX - this.drag.x;
      const dy = event.clientY - this.drag.y;
      this.drag = { x: event.clientX, y: event.clientY };
      this.moved({ x: x + dx, y: y + dy, scale });
    });
    canvas.addEventListener("pointerup", () => {
      this.drag = null;
      canvas.classList.remove("panning");
    });
    // Zoom popover: a number field.
    zoom.addEventListener("click", () => {
      popover.hidden = !popover.hidden;
      if (!popover.hidden) {
        const rect = zoom.getBoundingClientRect();
        popover.style.left = `${Math.min(rect.left, window.innerWidth - popover.offsetWidth - 8)}px`;
        popover.style.top = `${rect.bottom + 4}px`;
        number.focus();
        number.select();
      }
    });
    number.addEventListener("change", () => {
      const percent = Number(number.value);
      if (Number.isFinite(percent) && percent > 0) this.zoomTo(percent / 100);
    });
    number.addEventListener("keydown", (event) => {
      if (event.key === "Enter") number.blur();
    });
    document.addEventListener("pointerdown", (event) => {
      const target = event.target as HTMLElement;
      if (!popover.hidden && !popover.contains(target) && target !== zoom) popover.hidden = true;
    });
  }
}
