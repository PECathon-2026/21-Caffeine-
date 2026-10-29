import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
async function compile(entry) {
  const result = await build({
    entryPoints: [entry],
    bundle: true,
    write: false,
    format: "esm",
    platform: "node",
  });
  return import(
    "data:text/javascript;base64," +
      Buffer.from(result.outputFiles[0].text).toString("base64")
  );
}
const { cameraBox, clampCamera, fitCamera, focusCamera, zoomCamera } =
  await compile("web/guild/camera.ts");
const { workspaceBounds } = await compile("web/guild/layout.ts");
const { createActor, tickActor } = await compile("web/guild/scene.ts");
const { characterPixels, directionToward, roleOrder } = await compile(
  "web/guild/characters.ts",
);

test("camera: Fit guild contains all five workspaces on wide and narrow screens", () => {
  for (const aspect of [390 / 530, 900 / 650, 1600 / 872, 2560 / 800]) {
    const v = cameraBox(fitCamera(), aspect);
    for (const b of Object.values(workspaceBounds)) {
      assert.ok(b.x >= v.x && b.y >= v.y);
      assert.ok(
        b.x + b.width <= v.x + v.width && b.y + b.height <= v.y + v.height,
      );
    }
  }
});
test("camera: focus shows the whole selected room with breathing room", () => {
  for (const aspect of [0.65, 1.4, 2.2])
    for (const [name, b] of Object.entries(workspaceBounds)) {
      const focused = focusCamera(name, aspect),
        v = cameraBox(focused, aspect);
      assert.ok(focused.zoom > 1 && focused.zoom <= 3);
      assert.ok(b.x > v.x && b.y > v.y, `${name} top-left`);
      assert.ok(
        b.x + b.width < v.x + v.width && b.y + b.height < v.y + v.height,
        `${name} bottom-right`,
      );
    }
});
test("camera: repeated zoom and pan requests remain within scene limits", () => {
  for (const aspect of [0.7, 1.4, 2.5]) {
    let c = fitCamera();
    for (let i = 0; i < 50; i++) c = zoomCamera(c, 2, aspect);
    assert.equal(c.zoom, 4);
    c = clampCamera({ ...c, x: -10000, y: 10000 }, aspect);
    const b = cameraBox(c, aspect);
    assert.ok(b.x >= -150 && b.y + b.height <= 700);
    for (let i = 0; i < 50; i++) c = zoomCamera(c, 0.5, aspect);
    assert.deepEqual(c, fitCamera());
  }
});
test("camera: zoom holds the world point under the pointer steady", () => {
  const c = { x: 450, y: 325, zoom: 2 },
    u = 0.65,
    v = 0.4,
    aspect = 1.5;
  const before = cameraBox(c, aspect),
    after = cameraBox(zoomCamera(c, 1.25, aspect, u, v), aspect);
  assert.ok(
    Math.abs(before.x + u * before.width - after.x - u * after.width) < 1e-8,
  );
  assert.ok(
    Math.abs(before.y + v * before.height - after.y - v * after.height) < 1e-8,
  );
});
test("characters: a direction change turns in place before the first walking step", () => {
  const a = createActor("shogun", 0),
    position = { ...a.position };
  tickActor(a, 0.05);
  assert.equal(a.facing, "north");
  assert.equal(a.pose, "idle");
  assert.deepEqual(a.position, position);
  for (let i = 0; i < 6; i++) tickActor(a, 0.05);
  assert.ok(a.position.y < position.y);
  assert.equal(directionToward({ x: 0, y: 0 }, { x: -20, y: 2 }), "west");
  assert.equal(directionToward({ x: 0, y: 0 }, { x: 2, y: 20 }), "south");
});
test("characters: each role has a different front, side and rear drawing", () => {
  for (const role of roleOrder) {
    const faces = ["north", "south", "east", "west"].map((d) =>
      characterPixels(role, 0, "walking", undefined, d),
    );
    assert.equal(new Set(faces).size, 4);
    assert.notEqual(
      characterPixels(role, 0, "walking", undefined, "north"),
      characterPixels(role, 1, "walking", undefined, "north"),
    );
  }
});
