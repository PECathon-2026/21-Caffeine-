import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
const compiled = await build({
  entryPoints: ["web/guild/campaign.ts"],
  bundle: true,
  write: false,
  format: "esm",
  platform: "node",
});
const { initialCampaign, campaignDecision } = await import(
  "data:text/javascript;base64," +
    Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const mission = () => ({
  title: "Sample",
  request: "Sample",
  repo: "sample",
  phase: "testing",
  limit: 3,
  iteration: 1,
  running: true,
});
test("campaign: finding routes to the builder and revises the plan", () => {
  const r = campaignDecision("finding", mission(), initialCampaign());
  assert.equal(r.mission.phase, "repairing");
  assert.equal(r.mission.iteration, 2);
  assert.equal(r.delivered, true);
  assert.equal(r.campaign.messages.at(-2).from, "sentinel");
  assert.equal(r.campaign.messages.at(-2).to, "samurai");
  assert.match(r.campaign.decision, /Plan revised/);
});
test("campaign: repeated repair requests stop at the budget", () => {
  let r = { mission: mission(), campaign: initialCampaign() };
  for (let i = 0; i < 50; i++)
    r = campaignDecision("retry", r.mission, r.campaign);
  assert.equal(r.mission.iteration, 3);
  assert.equal(r.mission.running, false);
  assert.equal(r.campaign.hold, "budget");
  assert.equal(
    campaignDecision("continue", r.mission, r.campaign).campaign.hold,
    "budget",
  );
  r = campaignDecision("continue", { ...r.mission, limit: 4 }, r.campaign);
  assert.equal(r.campaign.hold, null);
});
test("campaign: clarification and stop require an explicit human continuation", () => {
  let r = campaignDecision("clarify", mission(), initialCampaign());
  assert.equal(r.campaign.hold, "clarification");
  assert.equal(r.mission.running, false);
  assert.equal(
    campaignDecision("continue", r.mission, r.campaign).campaign.hold,
    "clarification",
  );
  r = campaignDecision("continue", r.mission, {
    ...r.campaign,
    clarification: "Treat whitespace-only input as empty.",
  });
  assert.equal(r.campaign.hold, null);
  r = campaignDecision("stop", r.mission, r.campaign);
  assert.equal(r.campaign.hold, "stopped");
  assert.equal(
    campaignDecision("finding", r.mission, r.campaign).delivered,
    false,
  );
});
test("campaign: unavailable tools revise the approach, then escalate repeated failures", () => {
  let r = campaignDecision("tool-failure", mission(), initialCampaign());
  assert.equal(r.campaign.hold, null);
  assert.match(r.campaign.decision, /inspect files/);
  r = campaignDecision("tool-failure", r.mission, r.campaign);
  assert.equal(r.campaign.hold, "tool");
  assert.equal(r.mission.running, false);
});
