import { h, svgEl } from "../ui/dom.ts";
import { characterPixels } from "./characters.ts";
import {
  phases,
  phaseLabels,
  phaseOwner,
  roles,
  updates,
  type Mission,
  type Role,
} from "./mission.ts";
import type { Campaign, CampaignAction } from "./campaign.ts";

export type DetailView =
  "missions" | "guild" | "campaign" | "evidence" | "settings";
export interface DetailContext {
  view: DetailView;
  mission: Mission;
  campaign: Campaign;
  day: boolean;
  motionPaused: boolean;
  reducedMotion: boolean;
  filter: "all" | "findings";
  navigate: (view: DetailView | "overview") => void;
  act: (action: CampaignAction) => void;
  newMission: () => void;
  review: () => void;
  exportLog: () => void;
  agentDetails: (role: Role) => void;
  reset: () => void;
  toggleTheme: () => void;
  setMotion: (enabled: boolean) => void;
  setLimit: (limit: number) => void;
  setClarification: (value: string) => void;
  setFilter: (filter: "all" | "findings") => void;
}
const icons: Record<string, string> = {
  mission: "M5 21V3h14l-3 5 3 5H5",
  guild:
    "M8 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM2 21v-3a6 6 0 0 1 12 0v3m2-17a4 4 0 0 1 0 7m2 3a5 5 0 0 1 4 5v2",
  campaign: "M7 5v14m0-7h7a4 4 0 0 0 4-4M5 3h4v4H5zm0 14h4v4H5zM16 3h4v4h-4z",
  evidence: "M5 2h9l5 5v15H5ZM14 2v6h5M9 12h6M9 16h6",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2",
  arrow: "M5 12h14m-5-5 5 5-5 5",
  back: "M19 12H5m5-5-5 5 5 5",
  plus: "M12 5v14M5 12h14",
  check: "m5 12 4 4L19 6",
  shield: "m12 2 8 4v6c0 6-8 10-8 10S4 18 4 12V6ZM8 12l3 3 5-6",
  download: "M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2m0 16v2M2 12h2m16 0h2M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2",
  moon: "M20 14A9 9 0 0 1 10 4a9 9 0 1 0 10 10Z",
  play: "m8 4 13 8-13 8Z",
  pause: "M8 5v14M16 5v14",
  clock: "M12 7v5l3 2M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Z",
  repo: "M4 4h16v16H4ZM4 9h16M8 13h3m-3 3h7",
  info: "M12 10v7m0-11v1M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Z",
};
function glyph(name: string) {
  const s = svgEl("svg", {
    viewBox: "0 0 24 24",
    width: 18,
    height: 18,
    fill: "none",
    stroke: "currentColor",
    "stroke-width": 1.6,
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  });
  s.append(svgEl("path", { d: icons[name] ?? icons.mission! }));
  return s;
}
const span = (value: string, className = "") =>
  h("span", { class: className }, value);
const tag = (value: string, tone = "") => span(value, `desk-tag ${tone}`);
function action(label: string, run: () => void, icon?: string, kind = "") {
  return h(
    "button",
    {
      class: `desk-button ${kind}`,
      attrs: { type: "button" },
      on: { click: run },
    },
    icon && glyph(icon),
    span(label),
  );
}
function portrait(role: Role, large = false) {
  const s = svgEl("svg", {
    viewBox: "-6 -5 34 32",
    "aria-hidden": "true",
    "shape-rendering": "crispEdges",
  });
  s.innerHTML = characterPixels(role);
  return h(
    "span",
    { class: `desk-portrait ${role} ${large ? "large" : ""}` },
    s,
  );
}
const names = {
  missions: "Mission",
  guild: "Guild",
  campaign: "Campaign",
  evidence: "Evidence",
  settings: "Settings",
};
function pageHeading(
  kicker: string,
  title: string,
  description: string,
  ...actions: HTMLElement[]
) {
  return h(
    "div",
    { class: "desk-page-heading" },
    h(
      "div",
      null,
      span(kicker, "desk-eyebrow"),
      h("h1", { attrs: { tabindex: -1 } }, title),
      h("p", null, description),
    ),
    h("div", { class: "desk-page-actions" }, ...actions),
  );
}
function panelHeader(title: string, description?: string, right?: HTMLElement) {
  return h(
    "div",
    { class: "desk-panel-header" },
    h(
      "div",
      null,
      h("h2", null, title),
      description && h("p", null, description),
    ),
    right,
  );
}
function metric(label: string, value: string, sub: string, icon: string) {
  return h(
    "div",
    { class: "desk-metric" },
    h("div", null, span(label), glyph(icon)),
    h("strong", null, value),
    h("small", null, sub),
  );
}
function criteria(ctx: DetailContext) {
  const complete = phases.indexOf(ctx.mission.phase) >= 4;
  return h(
    "div",
    { class: "desk-criteria" },
    ...ctx.campaign.criteria.map((label, i) =>
      h(
        "div",
        { class: "desk-criterion" },
        span(
          complete ? "✓" : String(i + 1).padStart(2, "0"),
          `desk-check ${complete ? "done" : ""}`,
        ),
        h(
          "div",
          null,
          h("strong", null, label),
          h(
            "small",
            null,
            complete
              ? "Passing in the sample scenario"
              : "Waiting for verification",
          ),
        ),
      ),
    ),
  );
}
function phaseRail(ctx: DetailContext) {
  const p = phases.indexOf(ctx.mission.phase);
  const current =
    p === 0
      ? 0
      : p === 1 || p === 3
        ? 1
        : p === 2 || p === 4
          ? 2
          : p === 5
            ? 3
            : 4;
  return h(
    "ol",
    { class: "desk-phase-rail", attrs: { "aria-label": "Mission stages" } },
    ...["Plan", "Build", "Verify", "Review", "Approve"].map((name, i) =>
      h(
        "li",
        {
          class: i === current ? "current" : i < current ? "done" : "",
          attrs: { "aria-current": i === current ? "step" : undefined },
        },
        span(i < current ? "✓" : String(i + 1).padStart(2, "0")),
        h("strong", null, name),
        i === current &&
          tag(
            ctx.campaign.hold
              ? "On hold"
              : ctx.mission.phase === "complete"
                ? "Complete"
                : "Current",
          ),
      ),
    ),
  );
}
function missionPage(ctx: DetailContext) {
  const owner = roles.find((r) => r.id === phaseOwner[ctx.mission.phase])!;
  return h(
    "div",
    { class: "desk-page mission-page" },
    pageHeading(
      "THE MISSION",
      "Mission control",
      "A clear brief. A shared plan. Every step accounted for.",
      action("New mission", ctx.newMission, "plus"),
      action("Inspect change", ctx.review, "evidence", "primary"),
    ),
    h(
      "section",
      { class: "desk-mission-hero", attrs: { "aria-label": "Active mission" } },
      h(
        "div",
        { class: "desk-mission-top" },
        h(
          "div",
          { class: "desk-repo" },
          glyph("repo"),
          span(ctx.mission.repo),
          tag("LOCAL DEMO"),
        ),
        tag(
          ctx.campaign.hold ? "On hold" : phaseLabels[ctx.mission.phase],
          "warm",
        ),
      ),
      h("h2", null, ctx.mission.title),
      h("p", null, ctx.mission.request),
      h(
        "div",
        { class: "desk-mission-meta" },
        h(
          "div",
          null,
          portrait(owner.id),
          span(`${owner.name} is leading this stage`),
        ),
        h(
          "div",
          null,
          glyph("clock"),
          span(`Iteration ${ctx.mission.iteration} of ${ctx.mission.limit}`),
        ),
        h("div", null, glyph("shield"), span("Approval required")),
      ),
      phaseRail(ctx),
    ),
    h(
      "div",
      { class: "desk-two-column detail-grid" },
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "Acceptance criteria",
          "The definition of done for this sample mission.",
          tag(`${ctx.campaign.criteria.length} criteria`),
        ),
        criteria(ctx),
        h(
          "div",
          { class: "desk-panel-note" },
          glyph("info"),
          span(
            "Custom requests change the brief. The demo continues to use the email-validation scenario.",
          ),
        ),
      ),
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "The working plan",
          "Four responsibilities. One feedback loop.",
        ),
        h(
          "div",
          { class: "desk-plan" },
          ...[
            [
              "shogun",
              "Read the repository",
              "Map validators, the signup flow, and test conventions.",
            ],
            [
              "samurai",
              "Implement the change",
              "Preserve the contract and handle edge cases.",
            ],
            [
              "sentinel",
              "Test, verify, adapt",
              "Send failures back with a reproducible finding.",
            ],
            [
              "shadow",
              "Review before approval",
              "Inspect correctness and security, then request your seal.",
            ],
          ].map(([role, title, description]) =>
            h(
              "div",
              { class: "desk-plan-row" },
              portrait(role as Role),
              h("div", null, h("h3", null, title!), h("p", null, description!)),
              span(role === owner.id ? "Working" : "", "desk-role-state"),
            ),
          ),
        ),
      ),
    ),
  );
}
function guildPage(ctx: DetailContext) {
  const tools: Record<Role, string[]> = {
    shogun: ["Planning", "Code search", "Delegation"],
    samurai: ["File editing", "Terminal", "Repair"],
    sentinel: ["Tests", "Acceptance", "Verification"],
    shadow: ["Diff review", "Security", "Critique"],
  };
  return h(
    "div",
    { class: "desk-page guild-page" },
    pageHeading(
      "THE PEOPLE BEHIND THE WORK",
      "Meet the guild",
      "Four distinct perspectives, connected by a shared mission.",
      tag("04 agent roles", "warm"),
    ),
    h(
      "div",
      { class: "desk-agents agent-grid" },
      ...roles.map((role, i) => {
        const active =
          phaseOwner[ctx.mission.phase] === role.id &&
          !["complete", "approval"].includes(ctx.mission.phase);
        return h(
          "button",
          {
            class: `desk-agent ${role.id}`,
            attrs: { type: "button", "aria-label": `Meet ${role.name}` },
            on: {
              click: () => ctx.agentDetails(role.id),
              pointermove: (event: PointerEvent) => {
                if (ctx.day || ctx.reducedMotion || ctx.motionPaused) return;
                const card = event.currentTarget as HTMLElement,
                  rect = card.getBoundingClientRect();
                card.style.setProperty(
                  "--pointer-x",
                  `${event.clientX - rect.left}px`,
                );
                card.style.setProperty(
                  "--pointer-y",
                  `${event.clientY - rect.top}px`,
                );
              },
            },
          },
          h(
            "div",
            { class: "desk-agent-art" },
            span(`AGENT / 0${i + 1}`, "desk-agent-number"),
            portrait(role.id, true),
            span("↗", "desk-agent-open"),
          ),
          h(
            "div",
            { class: "desk-agent-copy" },
            h(
              "div",
              { class: "desk-agent-name" },
              h("h2", null, role.name),
              span(role.title),
            ),
            h("p", null, role.job),
            h(
              "div",
              { class: "desk-tool-tags" },
              ...tools[role.id].map((t) => span(t)),
            ),
          ),
          h(
            "div",
            { class: "desk-agent-status" },
            span(active ? "●" : "○", active ? "active-dot" : ""),
            span(active ? phaseLabels[ctx.mission.phase] : "Standing by"),
            span("View profile", "desk-agent-link"),
          ),
        );
      }),
    ),
    h(
      "section",
      { class: "desk-collaboration" },
      h(
        "div",
        null,
        glyph("campaign"),
        h("h2", null, "The handoff is part of the work"),
        h(
          "p",
          null,
          "Test failures and review findings return to the builder. The plan changes when new evidence arrives.",
        ),
      ),
      action("Open campaign", () => ctx.navigate("campaign"), "arrow"),
    ),
    h(
      "div",
      { class: "desk-disclosure" },
      glyph("info"),
      span(
        "These are interactive role previews. Live model connections are not enabled.",
      ),
    ),
  );
}
function budgetSelect(ctx: DetailContext, name: string) {
  return h(
    "select",
    {
      class: "desk-select",
      attrs: { "aria-label": name },
      on: {
        change: (e: Event) =>
          ctx.setLimit(Number((e.target as HTMLSelectElement).value)),
      },
    },
    ...[1, 2, 3, 4, 5].map((n) =>
      h(
        "option",
        { attrs: { value: n, selected: n === ctx.mission.limit } },
        `${n} iteration${n === 1 ? "" : "s"}`,
      ),
    ),
  );
}
function campaignPage(ctx: DetailContext) {
  const { campaign: c, mission: m } = ctx;
  const clarification = h("textarea", {
    attrs: {
      rows: 3,
      "aria-label": "Commander clarification",
      placeholder: "Record the clarification the guild needs…",
      maxlength: 2000,
    },
    on: {
      input: (e: Event) =>
        ctx.setClarification((e.target as HTMLTextAreaElement).value),
    },
  });
  clarification.value = c.clarification;
  return h(
    "div",
    { class: "desk-page campaign-page" },
    pageHeading(
      "COORDINATION & AUTONOMY",
      "The living campaign",
      "Findings become decisions. Decisions change what happens next.",
      action("Export demo log", ctx.exportLog, "download"),
    ),
    h(
      "div",
      { class: "desk-campaign-grid" },
      h(
        "section",
        {
          class: `desk-decision campaign-decision ${c.hold ? "held" : ""}`,
          attrs: { role: "status" },
        },
        h(
          "div",
          { class: "desk-decision-top" },
          glyph(c.hold ? "pause" : "campaign"),
          tag(
            c.hold ? "Needs your attention" : "Ready to coordinate",
            c.hold ? "warm" : "mint",
          ),
        ),
        span("CURRENT DECISION", "desk-eyebrow"),
        h(
          "h2",
          null,
          c.hold ? "The guild is waiting for you." : "Keep the mission moving.",
        ),
        h("p", null, c.decision),
        h(
          "div",
          { class: "desk-decision-actions" },
          action("Continue", () => ctx.act("continue"), "play", "primary"),
          action("Retry repair", () => ctx.act("retry"), "campaign"),
        ),
      ),
      h(
        "section",
        { class: "desk-panel desk-budget" },
        panelHeader("Bounded autonomy", "A clear limit for every campaign."),
        h(
          "div",
          { class: "desk-budget-value" },
          h("strong", null, String(m.iteration).padStart(2, "0")),
          span(`/ ${String(m.limit).padStart(2, "0")}`),
          h("small", null, "iterations used"),
        ),
        h(
          "div",
          {
            class: "desk-budget-track",
            attrs: {
              role: "meter",
              "aria-label": "Iterations used",
              "aria-valuenow": m.iteration,
              "aria-valuemin": 0,
              "aria-valuemax": Math.max(m.limit, m.iteration),
            },
          },
          ...Array.from({ length: m.limit }, (_, i) =>
            span("", i < m.iteration ? "used" : ""),
          ),
        ),
        h(
          "label",
          { class: "desk-budget-setting" },
          span("Iteration limit"),
          budgetSelect(ctx, "Campaign iteration limit"),
        ),
        h(
          "p",
          { class: "desk-small" },
          "Exhausted retries and repeated tool failures require a human decision.",
        ),
      ),
    ),
    c.hold === "clarification" &&
      h(
        "section",
        { class: "desk-panel desk-clarification" },
        panelHeader(
          "A question for the commander",
          "Should whitespace-only input count as empty or malformed?",
        ),
        clarification,
        action(
          "Record clarification & continue",
          () => ctx.act("continue"),
          "check",
          "primary",
        ),
      ),
    h(
      "div",
      { class: "desk-two-column" },
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "Shared mission record",
          "The latest handoffs and replies.",
          tag(
            `${c.messages.length} message${c.messages.length === 1 ? "" : "s"}`,
          ),
        ),
        h(
          "div",
          { class: "desk-messages" },
          ...[...c.messages]
            .reverse()
            .map((message) =>
              h(
                "article",
                { class: "desk-message" },
                message.from === "commander"
                  ? h("span", { class: "desk-commander" }, glyph("shield"))
                  : portrait(message.from),
                h(
                  "div",
                  null,
                  h(
                    "div",
                    { class: "desk-message-route" },
                    h("strong", null, message.from),
                    glyph("arrow"),
                    span(message.to),
                    h("small", null, `#${String(message.id).padStart(2, "0")}`),
                  ),
                  h("p", null, message.text),
                ),
              ),
            ),
        ),
      ),
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "Commander controls",
          "Choose the next action with the evidence at hand.",
        ),
        h(
          "div",
          { class: "desk-command-list" },
          action(
            "Send sample failure to Samurai",
            () => ctx.act("finding"),
            "evidence",
          ),
          action("Ask for clarification", () => ctx.act("clarify"), "guild"),
          action("Stop campaign", () => ctx.act("stop"), "pause", "danger"),
        ),
        h(
          "details",
          { class: "desk-experiment" },
          h("summary", null, "Test an unexpected condition"),
          h(
            "p",
            null,
            "The first unavailable tool revises the plan. A repeated failure pauses the campaign.",
          ),
          action(
            "Simulate unavailable tool",
            () => ctx.act("tool-failure"),
            "settings",
          ),
        ),
        h(
          "div",
          { class: "desk-panel-note" },
          glyph("shield"),
          span("Human approval is required before a mission can finish."),
        ),
      ),
    ),
    h(
      "div",
      { class: "desk-disclosure" },
      glyph("info"),
      span(
        "Coordination is simulated. No repository commands, file edits, or merges run here.",
      ),
    ),
  );
}
function evidencePage(ctx: DetailContext) {
  const p = phases.indexOf(ctx.mission.phase),
    failed = p === 2 || p === 3,
    passed = p >= 4;
  const events = updates
    .slice(0, p + 1)
    .map((update, index) => ({ ...update, index }))
    .filter((u) => ctx.filter === "all" || !!u.tone);
  const filter = (label: string, value: "all" | "findings") =>
    h(
      "button",
      {
        attrs: { type: "button", "aria-pressed": String(ctx.filter === value) },
        on: { click: () => ctx.setFilter(value) },
      },
      label,
    );
  return h(
    "div",
    { class: "desk-page evidence-page" },
    pageHeading(
      "THE RECORD",
      "Evidence & review",
      "Trace the change, inspect the findings, and make the final call.",
      action("Export demo log", ctx.exportLog, "download"),
      action("Open approval gate", ctx.review, "shield", "primary"),
    ),
    h(
      "div",
      { class: "desk-disclosure prominent" },
      glyph("info"),
      h(
        "p",
        null,
        h("strong", null, "Sample evidence. "),
        "The checks and events below are simulated. This prototype has not run repository tests.",
      ),
    ),
    h(
      "section",
      { class: "desk-metrics" },
      metric(
        "Sample checks",
        passed ? "12 / 12" : failed ? "11 / 12" : "—",
        passed
          ? "Illustrated as passing"
          : failed
            ? "One edge case needs repair"
            : "Waiting for the test stage",
        "check",
      ),
      metric(
        "Feedback loops",
        p >= 3 ? "01" : "00",
        "Failure → repair → verification",
        "campaign",
      ),
      metric(
        "Open findings",
        failed ? "01" : passed ? "00" : "—",
        failed ? "Surrounding whitespace" : "From the sample scenario",
        "evidence",
      ),
      metric(
        "Human approval",
        ctx.mission.phase === "complete"
          ? "Recorded"
          : p === 6
            ? "Required"
            : "Pending",
        "Your decision is the final gate",
        "shield",
      ),
    ),
    h(
      "div",
      { class: "desk-two-column evidence-columns" },
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "Run timeline",
          "An ordered record of the sample mission.",
          h(
            "div",
            {
              class: "desk-segmented",
              attrs: { "aria-label": "Filter run timeline" },
            },
            filter("All events", "all"),
            filter("Findings", "findings"),
          ),
        ),
        h(
          "div",
          { class: "desk-timeline" },
          ...events.map((u) =>
            h(
              "article",
              { class: `desk-event ${u.tone ?? ""}` },
              h("div", { class: "desk-event-marker" }, portrait(u.role)),
              h(
                "div",
                null,
                h(
                  "div",
                  { class: "desk-event-meta" },
                  span(roles.find((r) => r.id === u.role)!.name),
                  span(`00:${String(u.index * 7).padStart(2, "0")}`),
                ),
                h("h3", null, u.title),
                h("p", null, u.detail),
                u.tone &&
                  tag(
                    u.tone === "failure" ? "Repair requested" : "Plan adapted",
                    u.tone === "failure" ? "rose" : "warm",
                  ),
              ),
            ),
          ),
          !events.length &&
            h(
              "div",
              { class: "desk-empty" },
              glyph("check"),
              h("h3", null, "No findings in this stage"),
              h(
                "p",
                null,
                "Findings appear when the sample mission reaches testing and review.",
              ),
            ),
        ),
      ),
      h(
        "section",
        { class: "desk-panel" },
        panelHeader(
          "Acceptance & security",
          "The checks that lead to approval.",
        ),
        criteria(ctx),
        h(
          "div",
          { class: "desk-security" },
          glyph("shield"),
          h(
            "div",
            null,
            h("h3", null, "Security inspection"),
            h(
              "p",
              null,
              p >= 5
                ? "The sample reviewer reports no unresolved findings. A real security scan is not connected."
                : "The sample security review has not been reached yet.",
            ),
          ),
        ),
        action("Inspect proposed change", ctx.review, "arrow"),
      ),
    ),
  );
}
function settingsPage(ctx: DetailContext) {
  const row = (title: string, description: string, control: HTMLElement) =>
    h(
      "div",
      { class: "desk-setting" },
      h("div", null, h("h3", null, title), h("p", null, description)),
      control,
    );
  const theme = (label: string, day: boolean) =>
    h(
      "button",
      {
        attrs: { type: "button", "aria-pressed": String(ctx.day === day) },
        on: {
          click: () => {
            if (ctx.day !== day) ctx.toggleTheme();
          },
        },
      },
      glyph(day ? "sun" : "moon"),
      label,
    );
  const motion = h(
    "button",
    {
      class: "desk-switch",
      attrs: {
        type: "button",
        role: "switch",
        "aria-label": "Enable scenery animation",
        "aria-checked": String(!ctx.motionPaused),
      },
      on: { click: () => ctx.setMotion(ctx.motionPaused) },
    },
    span(""),
  );
  return h(
    "div",
    { class: "desk-page settings-page" },
    pageHeading(
      "MAKE IT YOURS",
      "Workspace settings",
      "Tune the environment. Define how far the guild can go.",
    ),
    h(
      "div",
      { class: "desk-settings-grid" },
      h(
        "div",
        { class: "desk-settings-stack settings-panel" },
        h(
          "section",
          { class: "desk-panel" },
          panelHeader("Appearance", "A comfortable space for focused work."),
          row(
            "Theme",
            "Choose the lighting for your workspace.",
            h(
              "div",
              { class: "desk-segmented" },
              theme("Light", true),
              theme("Dark", false),
            ),
          ),
          row(
            "Scenery animation",
            ctx.reducedMotion
              ? "Your device requests reduced motion. Animations stay off."
              : "Characters and camera transitions animate in dark mode only.",
            motion,
          ),
        ),
        h(
          "section",
          { class: "desk-panel" },
          panelHeader(
            "Autonomy",
            "Boundaries apply to the current demo mission.",
          ),
          row(
            "Iteration budget",
            "Maximum attempts before the guild needs your input.",
            budgetSelect(ctx, "Maximum demo iterations"),
          ),
          row(
            "Human approval",
            "A completed review still needs your final decision.",
            tag("Always required", "mint"),
          ),
        ),
        h(
          "section",
          { class: "desk-panel" },
          panelHeader("Connections", "What this workspace can access."),
          row(
            "Agent engine",
            "Role interactions use a scripted local scenario.",
            tag("Not connected"),
          ),
          row(
            "Repository access",
            "No commands, file changes, or merges are enabled.",
            tag("Preview only"),
          ),
        ),
        h(
          "section",
          { class: "desk-reset" },
          h(
            "div",
            null,
            h("h3", null, "Start a fresh campaign"),
            h("p", null, "Clear this demo’s progress and shared messages."),
          ),
          action("Reset demo", ctx.reset, undefined, "danger"),
        ),
      ),
      h(
        "aside",
        { class: "desk-settings-aside" },
        h("div", { class: "desk-aside-icon" }, glyph("shield")),
        span("YOUR WORKSPACE", "desk-eyebrow"),
        h("h2", null, "You set the boundaries."),
        h(
          "p",
          null,
          "The guild can suggest a next step. You decide when a held campaign resumes and when a change is approved.",
        ),
        h(
          "div",
          { class: "desk-aside-rule" },
          glyph("check"),
          span("Explicit retry limits"),
        ),
        h(
          "div",
          { class: "desk-aside-rule" },
          glyph("check"),
          span("Visible decision history"),
        ),
        h(
          "div",
          { class: "desk-aside-rule" },
          glyph("check"),
          span("Human approval gate"),
        ),
        h(
          "div",
          { class: "desk-aside-foot" },
          tag("LOCAL DEMO", "warm"),
          span("Settings reset when the page reloads."),
        ),
      ),
    ),
  );
}
export function detailPage(ctx: DetailContext) {
  return {
    missions: missionPage,
    guild: guildPage,
    campaign: campaignPage,
    evidence: evidencePage,
    settings: settingsPage,
  }[ctx.view](ctx);
}
export function detailSidebar(ctx: DetailContext) {
  return h(
    "aside",
    { class: "desk-sidebar" },
    h(
      "a",
      {
        class: "desk-brand",
        attrs: { href: "#overview", "aria-label": "Maestro command center" },
        on: {
          click: (event: Event) => {
            event.preventDefault();
            ctx.navigate("overview");
          },
        },
      },
      span("M", "guild-emblem"),
      h(
        "div",
        null,
        h("strong", null, "Maestro"),
        h("small", null, "THE ENGINEERING GUILD"),
      ),
    ),
    action(
      "Back to guild",
      () => ctx.navigate("overview"),
      "back",
      "desk-back",
    ),
    span("WORKSPACE", "desk-nav-label"),
    h(
      "nav",
      { class: "desk-nav", attrs: { "aria-label": "Workspace pages" } },
      ...Object.entries(names).map(([id, name]) =>
        h(
          "button",
          {
            class: id === ctx.view ? "selected" : "",
            attrs: {
              type: "button",
              "aria-label": name,
              "aria-current": id === ctx.view ? "page" : undefined,
            },
            on: { click: () => ctx.navigate(id as DetailView) },
          },
          glyph(id === "missions" ? "mission" : id),
          span(name),
          id === "guild" && span("04", "desk-nav-count"),
          id === "campaign" && ctx.campaign.hold && span("!", "desk-nav-alert"),
        ),
      ),
    ),
    h(
      "div",
      { class: "desk-sidebar-bottom" },
      h(
        "div",
        { class: "desk-workspace-stamp" },
        glyph("repo"),
        h(
          "div",
          null,
          h("strong", null, "Local workspace"),
          h("small", null, "UI preview · no connected agents"),
        ),
      ),
      action(
        ctx.day ? "Dark mode" : "Light mode",
        ctx.toggleTheme,
        ctx.day ? "moon" : "sun",
        "desk-theme",
      ),
    ),
  );
}
export function detailTopbar(ctx: DetailContext) {
  return h(
    "header",
    { class: "desk-topbar" },
    h(
      "div",
      { class: "desk-breadcrumb" },
      span("Workspace"),
      span("/"),
      h("strong", null, names[ctx.view]),
    ),
    h(
      "div",
      { class: "desk-topbar-status" },
      span("", "desk-status-dot"),
      span("Interactive demo"),
      h("span", { class: "desk-topbar-divider" }),
      glyph("repo"),
      span(ctx.mission.repo),
    ),
  );
}
