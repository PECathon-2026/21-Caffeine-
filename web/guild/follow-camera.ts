import {
  clampCamera,
  fitCamera,
  targetCamera,
  zoomCamera,
  type Camera,
  type CameraTarget,
} from "./camera.ts";
import { WORLD, type Workspace } from "./layout.ts";
import { phaseOwner, type Phase } from "./mission.ts";

export function workingStation(
  phase: Phase,
  handoff = false,
  held = false,
  engaged?: Workspace,
): CameraTarget {
  if (phase === "complete") return "guild";
  if (held || phase === "approval") return "campaign";
  if (handoff) return "courtyard";
  return engaged ?? phaseOwner[phase];
}

export interface CameraView {
  camera: Camera;
  target: CameraTarget;
  focus: CameraTarget;
  following: boolean;
  transitioning: boolean;
}
/** The scene's animation clock drives this controller; there is no second animation loop. */
export function createFollowCamera(
  changed: (view: CameraView) => void = () => {},
) {
  let camera = fitCamera(),
    aspect = WORLD.width / WORLD.height;
  let target: CameraTarget = "guild",
    focus: CameraTarget = "guild";
  let following = true,
    motion = false;
  let transition: { from: Camera; to: Camera; elapsed: number } | undefined;
  const snapshot = (): CameraView => ({
    camera: { ...camera },
    target,
    focus,
    following,
    transitioning: !!transition,
  });
  const publish = () => changed(snapshot());
  function move(to: Camera, nextFocus: CameraTarget, animated: boolean) {
    focus = nextFocus;
    const differs =
      Math.abs(camera.x - to.x) +
        Math.abs(camera.y - to.y) +
        Math.abs(camera.zoom - to.zoom) >
      0.001;
    transition =
      animated && differs ? { from: { ...camera }, to, elapsed: 0 } : undefined;
    if (!transition) camera = to;
    publish();
  }
  function takeOver() {
    following = false;
    transition = undefined;
  }
  return {
    snapshot,
    setTarget(next: CameraTarget) {
      if (next === target) return;
      target = next;
      if (following) move(targetCamera(target, aspect), target, motion);
      else publish();
    },
    setFollowing(enabled: boolean) {
      following = enabled;
      if (enabled) move(targetCamera(target, aspect), target, motion);
      else {
        transition = undefined;
        publish();
      }
    },
    setMotion(enabled: boolean) {
      motion = enabled;
      if (!enabled && transition) {
        camera = transition.to;
        transition = undefined;
        publish();
      }
    },
    resize(nextAspect: number) {
      if (
        !Number.isFinite(nextAspect) ||
        nextAspect <= 0 ||
        Math.abs(nextAspect - aspect) < 0.001
      )
        return;
      aspect = nextAspect;
      if (following) move(targetCamera(target, aspect), target, false);
      else {
        transition = undefined;
        camera = clampCamera(camera, aspect);
        publish();
      }
    },
    step(seconds: number) {
      if (!transition || !Number.isFinite(seconds) || seconds <= 0) return;
      transition.elapsed += seconds;
      const t = Math.min(1, transition.elapsed / 0.85),
        eased = t * t * (3 - 2 * t);
      const { from, to } = transition;
      camera = clampCamera(
        {
          x: from.x + (to.x - from.x) * eased,
          y: from.y + (to.y - from.y) * eased,
          zoom: from.zoom + (to.zoom - from.zoom) * eased,
        },
        aspect,
      );
      if (t === 1) {
        camera = to;
        transition = undefined;
      }
      publish();
    },
    fit() {
      takeOver();
      move(fitCamera(), "guild", false);
    },
    focusWorkspace(workspace: Workspace) {
      takeOver();
      move(targetCamera(workspace, aspect), workspace, false);
    },
    pan(dx: number, dy: number) {
      takeOver();
      camera = clampCamera(
        { ...camera, x: camera.x + dx, y: camera.y + dy },
        aspect,
      );
      publish();
    },
    zoom(factor: number, u = 0.5, v = 0.5) {
      takeOver();
      camera = zoomCamera(camera, factor, aspect, u, v);
      publish();
    },
  };
}
