import { h } from "../ui/dom.ts";
import { cameraBox, type CameraTarget } from "./camera.ts";
import { createFollowCamera } from "./follow-camera.ts";
import { WORLD, type Workspace } from "./layout.ts";

/** A single camera updates both SVGs; changing UI panels never resets the view. */
export function createNavigation(element: HTMLElement, changed: () => void) {
  let lastAspect = WORLD.width / WORLD.height;
  const aspect = () => {
    if (element.clientWidth && element.clientHeight)
      lastAspect = element.clientWidth / element.clientHeight;
    return lastAspect;
  };
  const cameraController = createFollowCamera(() => paint());
  const names: Record<CameraTarget, string> = {
    shogun: "Council",
    samurai: "Forge",
    sentinel: "Dojo",
    shadow: "Shadow Council",
    campaign: "Campaign",
    courtyard: "courtyard",
    guild: "whole guild",
  };
  const controls = h("div", {
    class: "camera-controls",
    attrs: { "aria-label": "Scene navigation" },
  });
  const percent = h(
    "output",
    { attrs: { "aria-label": "Scene zoom" } },
    "100%",
  );
  function paint() {
    const state = cameraController.snapshot();
    const box = cameraBox(state.camera, aspect());
    element
      .querySelectorAll(":scope > svg")
      .forEach((svg) =>
        svg.setAttribute(
          "viewBox",
          `${box.x} ${box.y} ${box.width} ${box.height}`,
        ),
      );
    element.dataset.zoom = state.camera.zoom.toFixed(3);
    element.dataset.focus = state.focus;
    element.dataset.autoFollow = String(state.following);
    element.dataset.cameraTarget = state.target;
    element.dataset.cameraTransitioning = String(state.transitioning);
    percent.textContent = `${Math.round(state.camera.zoom * 100)}%`;
    minus.disabled = state.camera.zoom <= 1;
    plus.disabled = state.camera.zoom >= 4;
    follow.setAttribute("aria-pressed", String(state.following));
    follow.title = state.following
      ? `Following ${names[state.target]}. Drag or zoom to take control.`
      : `Resume following ${names[state.target]}.`;
    changed();
    element.dispatchEvent(new Event("guild-camera-change"));
  }
  function sync() {
    cameraController.resize(aspect());
    paint();
  }
  const zoom = (factor: number, u = 0.5, v = 0.5) =>
    cameraController.zoom(factor, u, v);
  const pan = (dx: number, dy: number) => cameraController.pan(dx, dy);
  const fit = () => cameraController.fit();
  const focusWorkspace = (workspace: Workspace) =>
    cameraController.focusWorkspace(workspace);
  const control = (label: string, content: string, action: () => void) =>
    h(
      "button",
      {
        attrs: { type: "button", "aria-label": label, title: label },
        on: { click: action },
      },
      content,
    );
  const minus = control("Zoom out", "−", () => zoom(1 / 1.25));
  const plus = control("Zoom in", "+", () => zoom(1.25));
  const follow = control("Auto-follow active station", "Auto-follow", () =>
    cameraController.setFollowing(!cameraController.snapshot().following),
  );
  follow.className = "camera-follow";
  follow.prepend(
    h("span", { class: "follow-dot", attrs: { "aria-hidden": "true" } }),
  );
  percent.setAttribute("aria-live", "off");
  controls.append(
    follow,
    minus,
    percent,
    plus,
    control("Fit guild", "↙ Fit guild", fit),
  );
  element.tabIndex = 0;
  element.setAttribute("role", "region");
  element.setAttribute(
    "aria-label",
    "Guild scene. Drag to pan, scroll to zoom. Arrow keys pan; plus and minus zoom; zero fits the guild.",
  );
  element.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      const r = element.getBoundingClientRect();
      zoom(
        Math.exp(-Math.max(-100, Math.min(100, event.deltaY)) * 0.003),
        (event.clientX - r.left) / r.width,
        (event.clientY - r.top) / r.height,
      );
    },
    { passive: false },
  );
  let dragged = false;
  const pointers = new Map<number, { x: number; y: number }>();
  element.addEventListener("pointerdown", (event) => {
    if (
      event.button !== 0 ||
      (event.target as Element).closest('[role="button"],button,.scene-action')
    )
      return;
    dragged = false;
    element.focus({ preventScroll: true });
    element.setPointerCapture(event.pointerId);
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    element.classList.add("is-panning");
  });
  element.addEventListener("pointermove", (event) => {
    const last = pointers.get(event.pointerId);
    if (!last) return;
    const box = cameraBox(cameraController.snapshot().camera, aspect()),
      r = element.getBoundingClientRect();
    const dx = event.clientX - last.x,
      dy = event.clientY - last.y;
    if (Math.abs(dx) + Math.abs(dy) > 2) dragged = true;
    const other = [...pointers.entries()].find(
      ([id]) => id !== event.pointerId,
    )?.[1];
    if (other) {
      const before = Math.hypot(last.x - other.x, last.y - other.y),
        after = Math.hypot(event.clientX - other.x, event.clientY - other.y);
      if (before > 4)
        zoom(
          after / before,
          ((event.clientX + other.x) / 2 - r.left) / r.width,
          ((event.clientY + other.y) / 2 - r.top) / r.height,
        );
    } else pan((-dx * box.width) / r.width, (-dy * box.height) / r.height);
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  });
  const release = (event: PointerEvent) => {
    pointers.delete(event.pointerId);
    if (!pointers.size) element.classList.remove("is-panning");
  };
  element.addEventListener("pointerup", release);
  element.addEventListener("pointercancel", release);
  element.addEventListener(
    "click",
    (event) => {
      if (dragged) {
        event.stopPropagation();
        event.preventDefault();
        dragged = false;
      }
    },
    true,
  );
  element.addEventListener("keydown", (event) => {
    if (
      event.target !== element ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey
    )
      return;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-35, 0],
      ArrowRight: [35, 0],
      ArrowUp: [0, -35],
      ArrowDown: [0, 35],
    };
    if (moves[event.key]) {
      event.preventDefault();
      pan(...moves[event.key]!);
    } else if (event.key === "+" || event.key === "=") {
      event.preventDefault();
      zoom(1.25);
    } else if (event.key === "-") {
      event.preventDefault();
      zoom(1 / 1.25);
    } else if (event.key === "0") {
      event.preventDefault();
      fit();
    }
  });
  const observer =
    typeof ResizeObserver === "undefined"
      ? undefined
      : new ResizeObserver(sync);
  observer?.observe(element);
  return {
    controls,
    sync,
    fit,
    focusWorkspace,
    box: () => cameraBox(cameraController.snapshot().camera, aspect()),
    followTarget: (target: CameraTarget) => cameraController.setTarget(target),
    setMotion: (enabled: boolean) => cameraController.setMotion(enabled),
    step: (seconds: number) => cameraController.step(seconds),
    dispose: () => observer?.disconnect(),
  };
}
