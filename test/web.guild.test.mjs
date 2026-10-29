import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
const compiled = await build({
  entryPoints: ["web/guild/mission.ts"],
  bundle: true,
  write: false,
  format: "esm",
  platform: "node",
});
const { advance, approve, evidence, initialMission, phases } = await import(
  "data:text/javascript;base64," +
    Buffer.from(compiled.outputFiles[0].text).toString("base64")
);

test("guild demo: automation stops at approval, even after repeated advances", () => {
  let state = { ...initialMission(), phase: "planning", running: true };
  for (let i = 0; i < 30; i++) state = advance(state);
  assert.equal(state.phase, "approval");
  assert.equal(state.running, false);
  assert.equal(state.iteration, 2);
});
test("guild demo: approval cannot skip unfinished checks", () => {
  for (const phase of phases.filter((p) => p !== "approval")) {
    const state = { ...initialMission(), phase };
    assert.equal(approve(state).phase, phase);
  }
  assert.equal(
    approve({ ...initialMission(), phase: "approval" }).phase,
    "complete",
  );
});
test("guild demo: failure feeds back to a different role and consumes a retry", () => {
  const state = advance({
    ...initialMission(),
    phase: "testing",
    running: true,
  });
  assert.equal(state.phase, "repairing");
  assert.equal(state.iteration, 2);
  const log = evidence(state);
  assert.deepEqual(
    log.events.slice(-2).map((e) => e.role),
    ["sentinel", "samurai"],
  );
  assert.match(log.events.at(-1).detail, /finding changed the plan/);
});
test("guild demo: exhausted retry budget stops rather than pretending to pass", () => {
  const initial = {
    ...initialMission(),
    phase: "testing",
    limit: 1,
    running: true,
  };
  const state = advance(initial);
  assert.equal(state.phase, "testing");
  assert.equal(state.running, false);
  assert.equal(state.iteration, 1);
});
test("guild demo: exported evidence discloses simulation and excludes future results", () => {
  const log = evidence({ ...initialMission(), phase: "building" });
  assert.equal(log.simulated, true);
  assert.equal(log.kind, "maestro-ui-demo");
  assert.match(log.disclaimer, /No agents, commands, tests/);
  assert.equal(log.events.length, 2);
  assert.equal(
    log.events.some((e) => e.tone === "success"),
    false,
  );
});
