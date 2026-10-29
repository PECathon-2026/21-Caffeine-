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
const {
  walkNodes,
  walkEdges,
  routeTo,
  onWalkway,
  clearFootprint,
  handoffNodes,
} = await compile("web/guild/routes.ts");
const { guildTools, roleOrder, homeTools } = await compile(
  "web/guild/layout.ts",
);
const { createActor, tickActor, directActor, atDestination } =
  await compile("web/guild/scene.ts");
const { footBaseline, characterPixels } = await compile(
  "web/guild/characters.ts",
);
const { guildLayers } = await compile("web/guild/world.ts");
function validateRoute(start, points, label) {
  let a = start;
  for (const b of points) {
    const steps = Math.ceil(Math.hypot(b.x - a.x, b.y - a.y) / 2);
    for (let n = 0; n <= steps; n++) {
      const t = steps ? n / steps : 0,
        p = { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
      assert.ok(
        onWalkway(p),
        `${label} leaves the path at ${JSON.stringify(p)}`,
      );
      assert.ok(
        clearFootprint(p),
        `${label} intersects a wall or furniture at ${JSON.stringify(p)}`,
      );
    }
    a = b;
  }
}
test("routes: all 100 tool-to-tool journeys follow walkways and avoid solid footprints", () => {
  for (const source of guildTools)
    for (const target of guildTools) {
      const route = routeTo(source.stand, target.id);
      assert.ok(route, `${source.id} → ${target.id}`);
      validateRoute(source.stand, route, `${source.id} → ${target.id}`);
      assert.deepEqual(route.at(-1) ?? source.stand, target.stand);
    }
});
test("routes: commands issued mid-edge replan without teleporting or crossing scenery", () => {
  for (const role of roleOrder) {
    const a = createActor(role, 0);
    for (const target of [
      "relay-board",
      homeTools[role],
      "autonomy-console",
      homeTools[role],
      "relay-board",
      homeTools[role],
    ]) {
      const before = { ...a.position };
      assert.equal(directActor(a, target), true);
      assert.deepEqual(
        a.position,
        before,
        "replanning must not move the actor",
      );
      validateRoute(a.position, a.route, role);
      for (let frame = 0; frame < 73; frame++) {
        tickActor(a, 0.05);
        assert.ok(onWalkway(a.position));
        assert.ok(clearFootprint(a.position));
      }
    }
    for (let frame = 0; frame < 4000 && !atDestination(a); frame++)
      tickActor(a, 0.05);
    assert.ok(
      atDestination(a),
      `${role} eventually reaches its last requested tool`,
    );
  }
});
test("routes: entering and leaving every room uses its front doorway", () => {
  for (const role of roleOrder) {
    const route = routeTo(walkNodes[homeTools[role]], "relay-board");
    const door = walkNodes[`${role}-door`];
    assert.ok(
      route.some((p) => p.x === door.x && p.y === door.y),
      `${role} doorway must be visited`,
    );
  }
  for (const e of walkEdges) {
    const a = walkNodes[e.from],
      b = walkNodes[e.to];
    assert.ok(
      a.x === b.x || a.y === b.y,
      "no diagonal shortcuts through corners",
    );
  }
});
test("routes: handoff participants reach separate courtyard slots and return safely", () => {
  for (const [i, role] of roleOrder.entries()) {
    const a = createActor(role, 0),
      slot = i % 2 ? handoffNodes.sender : handoffNodes.receiver;
    directActor(a, "relay-board", slot);
    validateRoute(a.position, a.route, role);
    for (let frame = 0; frame < 4000 && !atDestination(a); frame++)
      tickActor(a, 0.05);
    assert.ok(atDestination(a));
    assert.deepEqual(a.position, walkNodes[slot]);
    directActor(a, homeTools[role]);
    validateRoute(a.position, a.route, `${role} returning`);
  }
});
test("routes: an invalid origin stops instead of drawing a straight path over a roof", () => {
  const a = createActor("samurai", 0);
  a.position = { x: 200, y: 350 };
  assert.equal(directActor(a, "relay-board"), false);
  assert.equal(a.routeBlocked, true);
  assert.equal(atDestination(a), false);
  assert.deepEqual(a.route, []);
  assert.deepEqual(a.position, { x: 200, y: 350 });
});
test("scene: every visible outdoor path comes from the routing graph", () => {
  const ground = guildLayers(true).ground;
  for (const e of walkEdges.filter((e) => e.outdoor))
    assert.ok(ground.includes(`data-walkway="${e.from}:${e.to}"`));
});
test("sprites: each role's soles touch its own ground baseline", () => {
  for (const role of roleOrder) {
    const pixels = characterPixels(role, 0, "idle");
    const bottoms = [
      ...pixels.matchAll(
        /<rect x="[^"]+" y="([^"]+)" width="[^"]+" height="([^"]+)"/g,
      ),
    ].map((m) => Number(m[1]) + Number(m[2]));
    assert.equal(
      Math.max(...bottoms),
      footBaseline(role),
      `${role} must not float above its ground anchor`,
    );
  }
});
