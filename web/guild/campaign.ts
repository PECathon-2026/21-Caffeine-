import type { Mission, Role } from "./mission.ts";
export type CampaignAction =
  "finding" | "retry" | "clarify" | "stop" | "continue" | "tool-failure";
export interface SharedMessage {
  id: number;
  from: Role | "commander";
  to: Role | "commander";
  text: string;
}
export interface Campaign {
  hold: null | "budget" | "clarification" | "stopped" | "tool";
  decision: string;
  messages: SharedMessage[];
  notes: string[];
  criteria: string[];
  checked: number[];
  clarification: string;
  failures: number;
  sequence: number;
}
export function initialCampaign(): Campaign {
  return {
    hold: null,
    decision: "Ready. Findings can reopen the plan; approval belongs to you.",
    messages: [
      {
        id: 1,
        from: "shogun",
        to: "samurai",
        text: "Preserve the existing validation rules. Share your change with Sentinel.",
      },
    ],
    notes: [],
    criteria: [
      "Accept valid addresses",
      "Reject malformed input",
      "Trim surrounding whitespace",
    ],
    checked: [],
    clarification: "",
    failures: 0,
    sequence: 1,
  };
}
export function recordMessage(
  c: Campaign,
  from: SharedMessage["from"],
  to: SharedMessage["to"],
  text: string,
): Campaign {
  return {
    ...c,
    sequence: c.sequence + 1,
    messages: [...c.messages, { id: c.sequence + 1, from, to, text }].slice(
      -40,
    ),
  };
}
/** UI simulation only. Bounded decisions are real frontend state; no agents or repo tools run. */
export function campaignDecision(
  action: CampaignAction,
  mission: Mission,
  state: Campaign,
  sender: Role = "sentinel",
) {
  let m = { ...mission, running: false },
    c = { ...state };
  if (action === "stop")
    return {
      mission: m,
      campaign: {
        ...c,
        hold: "stopped" as const,
        decision:
          "Campaign stopped by the commander. Continue explicitly to resume.",
      },
      delivered: false,
    };
  if (action === "clarify")
    return {
      mission: m,
      campaign: recordMessage(
        {
          ...c,
          hold: "clarification",
          decision:
            "Waiting for your clarification. Automatic steps are paused.",
        },
        "shogun",
        "commander",
        "Should whitespace-only input count as empty or malformed?",
      ),
      delivered: false,
    };
  if (action === "continue") {
    if (c.hold === "budget" && m.iteration >= m.limit)
      return { mission: m, campaign: c, delivered: false };
    if (c.hold === "clarification" && !c.clarification.trim())
      return { mission: m, campaign: c, delivered: false };
    c = recordMessage(
      {
        ...c,
        hold: null,
        failures: 0,
        decision: "The commander cleared the hold. The next step can proceed.",
      },
      "commander",
      "shogun",
      c.clarification.trim() || "Continue within the configured boundaries.",
    );
    return { mission: m, campaign: c, delivered: false };
  }
  if (c.hold) return { mission: m, campaign: c, delivered: false };
  if (m.phase === "complete")
    return {
      mission: m,
      campaign: {
        ...c,
        decision:
          "This demo is complete. Reset it before starting another campaign.",
      },
      delivered: false,
    };
  if (action === "tool-failure") {
    const failures = c.failures + 1;
    c = recordMessage(
      {
        ...c,
        failures,
        hold: failures >= 2 ? "tool" : null,
        decision:
          failures >= 2
            ? "Repeated tool failure. Paused for a human decision."
            : "Terminal unavailable. Revise the plan to inspect files before retrying.",
      },
      "samurai",
      "shogun",
      "Sample terminal tool unavailable; investigation required.",
    );
    return { mission: m, campaign: c, delivered: false };
  }
  if (m.iteration >= m.limit) {
    c = recordMessage(
      {
        ...c,
        hold: "budget",
        decision:
          "Retry budget exhausted. Raise the limit or stop; the guild will not loop indefinitely.",
      },
      "shogun",
      "commander",
      "The finding remains unresolved and no retries remain.",
    );
    return { mission: m, campaign: c, delivered: false };
  }
  m = { ...m, phase: "repairing", iteration: m.iteration + 1 };
  c = recordMessage(
    c,
    sender,
    "samurai",
    sender === "shadow"
      ? "Review finding: normalize input before validation."
      : "Sample failing case: surrounding whitespace is rejected.",
  );
  c = recordMessage(
    {
      ...c,
      decision:
        "Plan revised: trim input, add a regression case, then request independent verification.",
    },
    "samurai",
    "shogun",
    "Finding received. I am returning to the forge with a revised plan.",
  );
  return { mission: m, campaign: c, delivered: true };
}
