import {
  WORLD,
  meeting,
  workspaceBounds,
  type Bounds,
  type Workspace,
} from "./layout.ts";

export interface Camera {
  x: number;
  y: number;
  zoom: number;
}
export type CameraTarget = Workspace | "courtyard" | "guild";
export const fitCamera = (): Camera => ({
  x: WORLD.width / 2,
  y: WORLD.height / 2,
  zoom: 1,
});
export function cameraBox(
  camera: Camera,
  aspect = WORLD.width / WORLD.height,
): Bounds {
  const width = Math.max(WORLD.width, WORLD.height * aspect) / camera.zoom;
  const height = width / aspect;
  return { x: camera.x - width / 2, y: camera.y - height / 2, width, height };
}
export function clampCamera(
  camera: Camera,
  aspect = WORLD.width / WORLD.height,
): Camera {
  const zoom = Math.min(
    4,
    Math.max(1, Number.isFinite(camera.zoom) ? camera.zoom : 1),
  );
  const box = cameraBox({ ...camera, zoom }, aspect);
  const clamp = (n: number, size: number, total: number, margin: number) =>
    size >= total
      ? total / 2
      : Math.max(size / 2 - margin, Math.min(total + margin - size / 2, n));
  return {
    x: clamp(camera.x, box.width, WORLD.width, 150),
    y: clamp(camera.y, box.height, WORLD.height, 50),
    zoom,
  };
}
export function focusCamera(
  workspace: Workspace,
  aspect = WORLD.width / WORLD.height,
): Camera {
  return focusBounds(workspaceBounds[workspace], aspect);
}
function focusBounds(b: Bounds, aspect: number): Camera {
  const base = cameraBox(fitCamera(), aspect);
  return clampCamera(
    {
      x: b.x + b.width / 2,
      y: b.y + b.height / 2 + 12,
      zoom: Math.min(
        3,
        base.width / (b.width + 100),
        base.height / (b.height + 110),
      ),
    },
    aspect,
  );
}
export function targetCamera(target: CameraTarget, aspect: number): Camera {
  if (target === "guild") return fitCamera();
  if (target === "courtyard")
    return focusBounds(
      { x: meeting.x - 85, y: meeting.y - 75, width: 170, height: 150 },
      aspect,
    );
  return focusCamera(target, aspect);
}
/** Keep the point beneath the pointer steady while zooming. */
export function zoomCamera(
  camera: Camera,
  factor: number,
  aspect: number,
  u = 0.5,
  v = 0.5,
): Camera {
  const before = cameraBox(camera, aspect),
    zoom = Math.min(4, Math.max(1, camera.zoom * factor));
  const after = cameraBox({ ...camera, zoom }, aspect);
  return clampCamera(
    {
      x: camera.x + (u - 0.5) * (before.width - after.width),
      y: camera.y + (v - 0.5) * (before.height - after.height),
      zoom,
    },
    aspect,
  );
}
