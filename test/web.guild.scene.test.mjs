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
const {
  motionAllowed,
  moveToward,
  stations,
  roleOrder,
  characterPixels,
  animationProfiles,
} = await compile("web/guild/characters.ts");
const { createActor, tickActor, directActor, settleActor, atDestination } =
  await compile("web/guild/scene.ts");
test("scene: only dark, unpaused, visible, non-reduced mode can animate", () => {
  for (const dark of [false, true])
    for (const paused of [false, true])
      for (const reduced of [false, true])
        for (const visible of [false, true]) {
          assert.equal(
            motionAllowed(dark, paused, reduced, visible),
            dark && !paused && !reduced && visible,
          );
        }
});
test("scene: every character walks into its own workspace, thinks, then works", () => {
  for (const [i, role] of roleOrder.entries()) {
    const actor = createActor(role, i);
    const origin = { ...actor.position };
    for (let j = 0; j < 1000 && actor.route.length; j++) tickActor(actor, 0.05);
    assert.notDeepEqual(actor.position, origin);
    assert.deepEqual(actor.position, stations[role]);
    assert.equal(actor.route.length, 0);
    assert.equal(actor.pose, "thinking");
    tickActor(actor, 2.7);
    assert.equal(actor.pose, "working");
  }
});
test("scene: path movement never overshoots a station or produces NaN", () => {
  assert.deepEqual(moveToward({ x: 1, y: 1 }, { x: 4, y: 5 }, 50), {
    x: 4,
    y: 5,
  });
  assert.deepEqual(moveToward({ x: 4, y: 5 }, { x: 4, y: 5 }, 0), {
    x: 4,
    y: 5,
  });
  assert.deepEqual(moveToward({ x: 1, y: 1 }, { x: 4, y: 5 }, 2.5), {
    x: 2.5,
    y: 3,
  });
});
test("scene: walking and work change sprite limbs, with distinct role silhouettes", () => {
  for (const role of roleOrder) {
    assert.notEqual(
      characterPixels(role, 0, "walking"),
      characterPixels(role, 1, "walking"),
    );
    const frames = Array.from(
      { length: animationProfiles[role].frames },
      (_, i) => characterPixels(role, i, "working"),
    );
    assert.ok(new Set(frames).size >= 3, `${role} has a complete work cycle`);
  }
  assert.equal(new Set(roleOrder.map((role) => characterPixels(role))).size, 4);
});

const { workspaceBounds, boundsGap, guildTools, corridor } = await compile(
  "web/guild/layout.ts",
);
test("scene: all five workspace footprints keep a clear gap", () => {
  const entries = Object.entries(workspaceBounds);
  assert.equal(entries.length, 5);
  for (let i = 0; i < entries.length; i++)
    for (let j = i + 1; j < entries.length; j++)
      assert.ok(
        boundsGap(entries[i][1], entries[j][1]) >= 24,
        `${entries[i][0]} and ${entries[j][0]} must stay separated`,
      );
});
test("scene: tools and their work positions stay inside the assigned workspace", () => {
  assert.equal(guildTools.length, 10);
  for (const tool of guildTools)
    for (const point of [tool.anchor, tool.stand]) {
      const b = workspaceBounds[tool.workspace];
      assert.ok(
        point.x >= b.x &&
          point.x <= b.x + b.width &&
          point.y >= b.y &&
          point.y <= b.y + b.height,
        tool.id,
      );
    }
});
test("scene: role movement and work timing are distinct", () => {
  assert.equal(
    new Set(Object.values(animationProfiles).map((p) => p.speed)).size,
    4,
  );
  assert.equal(
    new Set(Object.values(animationProfiles).map((p) => p.rate)).size,
    4,
  );
  assert.equal(
    new Set(Object.values(animationProfiles).map((p) => p.action)).size,
    4,
  );
});
test("scene: circulation routes do not cross another workspace", () => {
  for (const role of roleOrder) {
    const route = corridor(role);
    for (let i = 1; i < route.length; i++)
      for (let t = 0; t <= 1; t += 0.04) {
        const p = {
          x: route[i - 1].x + (route[i].x - route[i - 1].x) * t,
          y: route[i - 1].y + (route[i].y - route[i - 1].y) * t,
        };
        for (const [name, b] of Object.entries(workspaceBounds))
          if (name !== role)
            assert.ok(
              !(
                p.x > b.x &&
                p.x < b.x + b.width &&
                p.y > b.y &&
                p.y < b.y + b.height
              ),
              `${role} path crossed ${name}`,
            );
      }
  }
});

test("scene: daylight settles interrupted journeys at the requested tool", () => {
  for (const tool of guildTools) {
    const actor = createActor(tool.role, 0);
    directActor(actor, tool.id);
    for (let i = 0; i < 17; i++) tickActor(actor, 0.05);
    settleActor(actor);
    assert.deepEqual(actor.position, tool.stand, tool.id);
    assert.ok(atDestination(actor));
    assert.equal(actor.route.length, 0);
    assert.equal(actor.pose, "working");
    assert.equal(actor.turnRemaining, 0);
    assert.equal(
      actor.facing,
      tool.id === "scanner"
        ? "east"
        : tool.id === "evidence-shelf"
          ? "north"
          : "south",
    );
  }
});

test("scene: still-mode commands update tool, position and pose immediately", () => {
  const actor = createActor("samurai", 0);
  directActor(actor, "quench", "quench", true);
  assert.equal(actor.tool, "quench");
  assert.equal(actor.pose, "working");
  assert.ok(atDestination(actor));
  directActor(actor, "anvil", "anvil", true);
  assert.equal(actor.tool, "anvil");
  assert.ok(atDestination(actor));
});
