/** UI-only scenario. These events are fictional, not tool or model execution results. */
export type Phase =
  | "planning"
  | "building"
  | "testing"
  | "repairing"
  | "verifying"
  | "reviewing"
  | "approval"
  | "complete";
export type Role = "shogun" | "samurai" | "sentinel" | "shadow";
export const roles: {
  id: Role;
  name: string;
  title: string;
  job: string;
  color: string;
  mark: string;
}[] = [
  {
    id: "shogun",
    name: "Shogun",
    title: "The strategist",
    job: "Plans the mission, delegates work, and revises the approach.",
    color: "#9c8146",
    mark: "将",
  },
  {
    id: "samurai",
    name: "Samurai",
    title: "The builder",
    job: "Reads the repository, implements changes, and responds to findings.",
    color: "#b96447",
    mark: "侍",
  },
  {
    id: "sentinel",
    name: "Sentinel",
    title: "The verifier",
    job: "Runs checks and independently verifies acceptance criteria.",
    color: "#5e8068",
    mark: "守",
  },
  {
    id: "shadow",
    name: "Shadow",
    title: "The reviewer",
    job: "Inspects correctness, security, and regressions before approval.",
    color: "#82738f",
    mark: "影",
  },
];
export const phases: Phase[] = [
  "planning",
  "building",
  "testing",
  "repairing",
  "verifying",
  "reviewing",
  "approval",
  "complete",
];
export const phaseLabels: Record<Phase, string> = {
  planning: "Planning",
  building: "Building",
  testing: "Testing",
  repairing: "Repairing",
  verifying: "Verifying",
  reviewing: "Reviewing",
  approval: "Awaiting approval",
  complete: "Demo complete",
};
export const phaseOwner: Record<Phase, Role> = {
  planning: "shogun",
  building: "samurai",
  testing: "sentinel",
  repairing: "samurai",
  verifying: "sentinel",
  reviewing: "shadow",
  approval: "shogun",
  complete: "shogun",
};
export const updates: {
  role: Role;
  title: string;
  detail: string;
  tone?: string;
}[] = [
  {
    role: "shogun",
    title: "A plan takes shape",
    detail:
      "Three acceptance criteria recorded. Samurai, preserve the existing validation rules.",
  },
  {
    role: "samurai",
    title: "First implementation ready",
    detail:
      "Added email validation and four edge-case tests. Handing the change to Sentinel.",
  },
  {
    role: "sentinel",
    title: "An edge case needs attention",
    detail:
      "The whitespace case failed. Expected a valid email after trimming; received false.",
    tone: "failure",
  },
  {
    role: "samurai",
    title: "A new approach, backed by evidence",
    detail:
      "Sentinel’s finding changed the plan: normalize whitespace before validation. Added a regression case.",
    tone: "repair",
  },
  {
    role: "sentinel",
    title: "Independent verification complete",
    detail:
      "The illustrative rerun passes all 12 checks. Acceptance criteria have been rechecked.",
    tone: "success",
  },
  {
    role: "shadow",
    title: "Review sealed",
    detail:
      "Illustrative review: no unresolved findings. The diff is ready for a human to inspect.",
    tone: "success",
  },
  {
    role: "shogun",
    title: "Your seal is required",
    detail:
      "The guild has reached its execution boundary. Review the evidence before approving this demo.",
  },
  {
    role: "shogun",
    title: "The campaign is complete",
    detail: "Demo approval recorded. No repository was modified or merged.",
    tone: "success",
  },
];
export interface Mission {
  title: string;
  request: string;
  repo: string;
  phase: Phase;
  limit: number;
  iteration: number;
  running: boolean;
}
export function initialMission(): Mission {
  return {
    title: "Make every email count.",
    request:
      "Add email validation to the signup form. Accept valid addresses, reject malformed input, and handle surrounding whitespace without changing existing behavior.",
    repo: "acme / little-forms",
    phase: "building",
    limit: 3,
    iteration: 1,
    running: false,
  };
}
export function advance(mission: Mission): Mission {
  if (mission.phase === "approval" || mission.phase === "complete")
    return { ...mission, running: false };
  if (mission.phase === "testing" && mission.iteration >= mission.limit)
    return { ...mission, running: false };
  const phase = phases[phases.indexOf(mission.phase) + 1]!;
  return {
    ...mission,
    phase,
    iteration:
      phase === "repairing" ? mission.iteration + 1 : mission.iteration,
    running: mission.running && phase !== "approval",
  };
}
export function approve(mission: Mission): Mission {
  return mission.phase === "approval"
    ? { ...mission, phase: "complete", running: false }
    : mission;
}
export function evidence(mission: Mission) {
  return {
    kind: "maestro-ui-demo",
    simulated: true,
    disclaimer:
      "Illustrative UI scenario. No agents, commands, tests, security scans, repository changes or merges were executed.",
    mission: { ...mission, running: false },
    events: updates.slice(0, phases.indexOf(mission.phase) + 1),
  };
}
