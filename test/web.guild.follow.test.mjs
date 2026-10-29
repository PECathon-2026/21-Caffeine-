import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
async function compile(entry) {
  const r = await build({
    entryPoints: [entry],
    bundle: true,
    write: false,
    format: "esm",
    platform: "node",
  });
  return import(
    "data:text/javascript;base64," +
      Buffer.from(r.outputFiles[0].text).toString("base64")
  );
}
const { createFollowCamera, workingStation } = await compile(
  "web/guild/follow-camera.ts",
);
const { targetCamera, fitCamera } = await compile("web/guild/camera.ts");
const { WORLD } = await compile("web/guild/layout.ts");
const aspect = WORLD.width / WORLD.height;

test("auto-follow: phase changes select the station doing the work", () => {
  const camera = createFollowCamera();
  for (const [phase, target] of [
    ["planning", "shogun"],
    ["building", "samurai"],
    ["testing", "sentinel"],
    ["repairing", "samurai"],
    ["verifying", "sentinel"],
    ["reviewing", "shadow"],
    ["approval", "campaign"],
    ["complete", "guild"],
  ]) {
    assert.equal(workingStation(phase), target);
    camera.setTarget(workingStation(phase));
    assert.equal(camera.snapshot().following, true);
    assert.deepEqual(camera.snapshot().camera, targetCamera(target, aspect));
  }
});
test("auto-follow: handoffs, holds, and selected tools have explicit destinations", () => {
  assert.equal(workingStation("repairing", true), "courtyard");
  assert.equal(workingStation("building", false, true), "campaign");
  assert.equal(workingStation("building", true, true), "campaign");
  assert.equal(workingStation("testing", false, false, "shadow"), "shadow");
  assert.equal(workingStation("complete", false, false, "shadow"), "guild");
});
test("auto-follow: dark-mode transitions ease to the target without restarting each frame", () => {
  const camera = createFollowCamera();
  camera.setMotion(true);
  camera.setTarget("samurai");
  assert.deepEqual(camera.snapshot().camera, fitCamera());
  assert.equal(camera.snapshot().transitioning, true);
  camera.step(0.4);
  assert.ok(camera.snapshot().camera.zoom > 1);
  assert.ok(
    camera.snapshot().camera.zoom < targetCamera("samurai", aspect).zoom,
  );
  for (let i = 0; i < 30; i++) {
    camera.setTarget("samurai");
    camera.step(0.05);
  }
  assert.equal(camera.snapshot().transitioning, false);
  assert.deepEqual(camera.snapshot().camera, targetCamera("samurai", aspect));
});
test("auto-follow: light, pause, and reduced-motion changes end camera animation immediately", () => {
  const camera = createFollowCamera();
  camera.setMotion(true);
  camera.setTarget("sentinel");
  camera.step(0.2);
  camera.setMotion(false);
  const still = camera.snapshot();
  assert.equal(still.transitioning, false);
  assert.deepEqual(still.camera, targetCamera("sentinel", aspect));
  camera.step(10);
  assert.deepEqual(camera.snapshot(), still);
  camera.setTarget("shadow");
  assert.equal(camera.snapshot().transitioning, false);
  assert.deepEqual(camera.snapshot().camera, targetCamera("shadow", aspect));
});
test("auto-follow: every manual camera control wins until follow is explicitly resumed", () => {
  for (const action of [
    (c) => c.zoom(1.25),
    (c) => c.pan(30, -20),
    (c) => c.focusWorkspace("shadow"),
    (c) => c.fit(),
  ]) {
    const camera = createFollowCamera();
    camera.setMotion(true);
    camera.setTarget("samurai");
    camera.step(0.3);
    action(camera);
    const manual = camera.snapshot().camera;
    assert.equal(camera.snapshot().following, false);
    assert.equal(camera.snapshot().transitioning, false);
    camera.setTarget("sentinel");
    camera.step(5);
    assert.deepEqual(camera.snapshot().camera, manual);
    camera.setFollowing(true);
    camera.step(1);
    assert.deepEqual(
      camera.snapshot().camera,
      targetCamera("sentinel", aspect),
    );
  }
});
test("auto-follow: a new target interrupts smoothly, and resize keeps the active room framed", () => {
  const camera = createFollowCamera();
  camera.setMotion(true);
  camera.setTarget("samurai");
  camera.step(0.4);
  const halfway = camera.snapshot().camera;
  camera.setTarget("shadow");
  assert.deepEqual(camera.snapshot().camera, halfway);
  camera.step(1);
  assert.deepEqual(camera.snapshot().camera, targetCamera("shadow", aspect));
  camera.resize(0.75);
  assert.deepEqual(camera.snapshot().camera, targetCamera("shadow", 0.75));
  assert.equal(camera.snapshot().following, true);
});
