import { h, svgEl, adoptCss } from "../ui/dom.ts";
import css from "./styles.css";
import cockpitCss from "./cockpit.css";
import detailCss from "./detail-pages.css";
import {
  detailPage,
  detailSidebar,
  detailTopbar,
  type DetailView,
  type DetailContext,
} from "./detail-pages.ts";
import { createGuildScene } from "./scene.ts";
import { guildTools, type ToolId, type Workspace } from "./layout.ts";
import { characterPixels } from "./characters.ts";
import {
  initialCampaign,
  campaignDecision,
  recordMessage,
  type CampaignAction,
} from "./campaign.ts";
import {
  advance,
  approve,
  evidence,
  initialMission,
  phaseLabels,
  phaseOwner,
  phases,
  roles,
  updates,
  type Mission,
  type Role,
} from "./mission.ts";

type View = "overview" | DetailView;
const paths: Record<string, string> = {
  grid: "M3 3h6v6H3zM15 3h6v6h-6zM3 15h6v6H3zM15 15h6v6h-6z",
  flag: "M5 22V3m0 0h14l-3 5 3 5H5",
  users:
    "M8 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm-6 9v-3a6 6 0 0 1 12 0v3m2-16a4 4 0 0 1 0 7m2 3a5 5 0 0 1 4 5v1",
  file: "M5 2h9l5 5v15H5zM14 2v6h5M9 12h6M9 16h6",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm0-6v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2",
  arrow: "M4 12h16m-6-6 6 6-6 6",
  plus: "M12 4v16M4 12h16",
  check: "m5 12 4 4L19 6",
  branch:
    "M7 6v11m0-7h7a4 4 0 0 0 4-4M7 3a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm0 14a2 2 0 1 0 0 4 2 2 0 0 0 0-4ZM18 2a2 2 0 1 0 0 4 2 2 0 0 0 0-4Z",
  lock: "M6 10h12v11H6zM8 10V6a4 4 0 0 1 8 0v4m-4 5v2",
  play: "m8 4 13 8-13 8Z",
  pause: "M8 5v14M16 5v14",
  close: "m6 6 12 12M6 18 18 6",
  expand: "M4 9V4h5m6 0h5v5M4 15v5h5m6 0h5v-5",
  moon: "M20 14A9 9 0 0 1 10 4a9 9 0 1 0 10 10Z",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2m0 16v2M2 12h2m16 0h2M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2",
  shield: "m12 2 8 4v6c0 6-8 10-8 10S4 18 4 12V6ZM8 12l3 3 5-6",
  download: "M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5",
};
function icon(name: string) {
  const svg = svgEl("svg", {
    viewBox: "0 0 24 24",
    width: 18,
    height: 18,
    fill: "none",
    stroke: "currentColor",
    "stroke-width": 1.5,
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  });
  svg.append(svgEl("path", { d: paths[name] ?? paths.grid! }));
  return svg;
}
const text = (s: string, cls = "") => h("span", { class: cls }, s);
const button = (label: string, action: () => void, cls = "", glyph?: string) =>
  h(
    "button",
    { class: `btn ${cls}`, attrs: { type: "button" }, on: { click: action } },
    glyph && icon(glyph),
    label,
  );
const badge = (s: string, cls = "") => text(s, `badge ${cls}`);
const avatar = (id: Role, size = "") => {
  const sprite = svgEl("svg", {
    viewBox: "-6 -4 34 30",
    width: 32,
    height: 34,
    "aria-hidden": "true",
    "shape-rendering": "crispEdges",
  });
  sprite.innerHTML = characterPixels(id);
  return h(
    "span",
    { class: `avatar ${id} ${size}`, attrs: { "aria-hidden": "true" } },
    sprite,
  );
};

export function mountGuild() {
  adoptCss(css);
  adoptCss(cockpitCss);
  adoptCss(detailCss);
  let evidenceFilter: "all" | "findings" = "all";
  let lastRenderedView: View | undefined;
  let mission: Mission = initialMission(),
    view: View = "overview";
  let timer: ReturnType<typeof setInterval> | undefined;
  const motionPreference = matchMedia("(prefers-reduced-motion: reduce)");
  let reducedMotion = motionPreference.matches,
    motionPaused = false,
    day = false;
  let campaign = initialCampaign(),
    selectedTool: ToolId | undefined,
    checksPreview = false;
  let focusedWorkspace: Workspace | undefined;
  const scene = createGuildScene((role) => agentDetails(role), openTool);
  scene.element.addEventListener("guild-camera-change", () => {
    focusedWorkspace =
      scene.element.dataset.focus === "guild" ||
      scene.element.dataset.focus === "courtyard"
        ? undefined
        : (scene.element.dataset.focus as Workspace);
    root
      .querySelectorAll(".workspace-nav button")
      .forEach((b) =>
        b.setAttribute(
          "aria-pressed",
          String(b.getAttribute("data-workspace") === focusedWorkspace),
        ),
      );
  });
  function toggleTheme() {
    day = !day;
    render();
  }
  motionPreference.addEventListener("change", (event) => {
    reducedMotion = event.matches;
    render();
  });
  let activityOpen = false;
  const compactView = matchMedia("(max-width: 720px)");
  compactView.addEventListener("change", (event) => {
    if (event.matches) {
      activityOpen = false;
      render();
    }
  });
  let dialog: HTMLDialogElement | undefined;
  const root = h("div", { class: "maestro" }),
    live = h("div", {
      class: "sr-only",
      attrs: { "aria-live": "polite", "aria-atomic": "true" },
    });
  document.body.replaceChildren(root, live);
  function stop() {
    clearInterval(timer);
    timer = undefined;
    mission.running = false;
  }
  function notify(message: string) {
    live.textContent = message;
    document.querySelector(".toast")?.remove();
    const toast = h(
      "div",
      { class: "toast", attrs: { role: "status" } },
      icon("check"),
      message,
    );
    document.body.append(toast);
    setTimeout(() => toast.remove(), 5000);
  }
  function step() {
    if (campaign.hold) {
      notify("The campaign is on hold. Open Autonomy controls to continue.");
      return;
    }
    const next = advance(mission);
    if (next.phase === mission.phase && mission.phase === "testing") {
      stop();
      render();
      notify(
        "Iteration limit reached. Raise it in Settings or reset the demo.",
      );
      return;
    }
    mission = next;
    if (!mission.running) stop();
    live.textContent = `Demo: ${phaseLabels[mission.phase]}. ${updates[phases.indexOf(mission.phase)]!.title}`;
    render();
  }
  function play() {
    if (campaign.hold) {
      selectedTool = "autonomy-console";
      render();
      return;
    }
    if (mission.running) {
      stop();
      render();
      return;
    }
    if (mission.phase === "approval") {
      review();
      return;
    }
    if (mission.phase === "complete") {
      reset();
      return;
    }
    mission.running = true;
    timer = setInterval(step, 10000);
    render();
  }
  function reset() {
    stop();
    mission = { ...initialMission(), limit: mission.limit, phase: "planning" };
    campaign = initialCampaign();
    selectedTool = undefined;
    checksPreview = false;
    render();
    notify("Demo reset. The guild is ready for a new campaign.");
  }
  function navigate(v: View) {
    view = v;
    selectedTool = undefined;
    scene.closeAction();
    render();
    window.scrollTo?.(0, 0);
    root
      .querySelector<HTMLElement>(".desk-page h1")
      ?.focus({ preventScroll: true });
  }
  function exportLog() {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify({ ...evidence(mission), campaign }, null, 2)], {
        type: "application/json",
      }),
    );
    h("a", {
      attrs: { href: url, download: "maestro-demo-run.json" },
    }).click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    notify("Demo log exported. All entries are marked as simulated.");
  }
  function openDialog(
    title: string,
    eyebrow: string,
    content: HTMLElement,
    wide = false,
  ) {
    dialog?.close();
    const focus = document.activeElement as HTMLElement | null;
    const d = h("dialog", {
      class: `guild-dialog ${wide ? "wide" : ""}`,
      attrs: { "aria-labelledby": "dialog-title" },
    });
    dialog = d;
    const close = button("", () => d.close(), "icon-btn", "close");
    close.setAttribute("aria-label", "Close dialog");
    d.append(
      h(
        "div",
        { class: "dialog-header" },
        h(
          "div",
          null,
          text(eyebrow, "eyebrow"),
          h("h2", { attrs: { id: "dialog-title" } }, title),
        ),
        close,
      ),
      content,
    );
    d.addEventListener("click", (e) => {
      if (e.target === d) {
        const b = d.getBoundingClientRect();
        if (
          e.clientX < b.left ||
          e.clientX > b.right ||
          e.clientY < b.top ||
          e.clientY > b.bottom
        )
          d.close();
      }
    });
    d.addEventListener("close", () => {
      d.remove();
      if (dialog === d) dialog = undefined;
      if (focus?.isConnected) focus.focus();
    });
    document.body.append(d);
    d.showModal();
  }
  function newMission() {
    const request = h("textarea", {
      attrs: {
        id: "request",
        rows: 4,
        required: true,
        placeholder: "Describe the change your guild should make…",
        maxlength: 2000,
      },
    });
    const repo = h("input", {
      attrs: {
        id: "repository",
        value: "acme / little-forms",
        required: true,
        maxlength: 160,
      },
    });
    const form = h(
      "form",
      { class: "mission-form" },
      h(
        "p",
        { class: "muted" },
        "Describe the outcome you want. This local demo previews how the guild plans, builds, and reviews it.",
      ),
      h("label", { attrs: { for: "repository" } }, "Repository label"),
      repo,
      h(
        "label",
        { attrs: { for: "request" } },
        "What should the guild change?",
      ),
      request,
      h(
        "div",
        { class: "callout" },
        icon("flag"),
        h(
          "div",
          null,
          h("strong", null, "UI demo only"),
          h(
            "p",
            null,
            "Your request labels the mission. The sample email-validation plan and evidence remain illustrative.",
          ),
        ),
      ),
      h(
        "button",
        { class: "btn primary", attrs: { type: "submit" } },
        "Create demo mission",
        icon("arrow"),
      ),
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      if (!request.value.trim() || !repo.value.trim()) {
        request.setCustomValidity("Enter a request and repository label.");
        request.reportValidity();
        return;
      }
      stop();
      mission = {
        ...initialMission(),
        title: request.value.trim().slice(0, 85),
        request: request.value.trim(),
        repo: repo.value.trim(),
        phase: "planning",
        limit: mission.limit,
      };
      campaign = initialCampaign();
      checksPreview = false;
      selectedTool = undefined;
      view = "overview";
      dialog?.close();
      render();
      notify("Demo mission created. Its plan and evidence are illustrative.");
    });
    request.addEventListener("input", () => request.setCustomValidity(""));
    openDialog("Start a new mission", "MISSION BRIEF", form);
  }
  function agentDetails(id: Role) {
    const r = roles.find((r) => r.id === id)!;
    const tools: Record<Role, string[]> = {
      shogun: ["Read & search", "Mission plan", "Agent messages"],
      samurai: ["Read & search", "File edit", "Terminal commands"],
      sentinel: ["Test runner", "Acceptance checklist", "Agent messages"],
      shadow: ["Diff review", "Secret scan", "Security checklist"],
    };
    const messages = updates
      .slice(0, phases.indexOf(mission.phase) + 1)
      .filter((u) => u.role === id);
    openDialog(
      `${r.name} · ${r.title}`,
      "MEET YOUR GUILD",
      h(
        "div",
        { class: "dialog-body" },
        h(
          "div",
          { class: "agent-profile" },
          avatar(id, "large"),
          h("p", null, r.job),
        ),
        h("h3", null, "Planned tools"),
        h("div", { class: "chips" }, ...tools[id].map((t) => badge(t))),
        h("h3", null, "Mission messages"),
        ...messages.map((m) =>
          h(
            "div",
            { class: "message-card" },
            h("strong", null, m.title),
            h("p", null, m.detail),
          ),
        ),
        !messages.length &&
          h("p", { class: "muted" }, "Waiting for the next handoff."),
        h(
          "p",
          { class: "fine-print" },
          "Simulated agent. No model or tool is connected in this version.",
        ),
      ),
    );
  }
  function review() {
    const done = phases.indexOf(mission.phase) >= 4;
    const yes = button(
      "Approve demo change",
      () => {
        if (mission.phase !== "approval") return;
        mission = approve(mission);
        stop();
        dialog?.close();
        render();
        notify("Demo approval recorded. No files were changed or merged.");
      },
      "primary",
      "shield",
    );
    yes.disabled = mission.phase !== "approval";
    openDialog(
      "Review the proposed change",
      "HUMAN APPROVAL",
      h(
        "div",
        { class: "dialog-body" },
        h(
          "p",
          { class: "muted" },
          "Inspect the proposed change and its evidence. The guild cannot cross this boundary on its own.",
        ),
        h(
          "div",
          { class: "chips" },
          badge("SIMULATED DIFF", "amber"),
          badge(
            done ? "12 / 12 sample checks" : "Checks pending",
            done ? "green" : "",
          ),
        ),
        h(
          "div",
          { class: "diff" },
          h(
            "div",
            { class: "diff-title" },
            icon("file"),
            "src / validation.ts",
          ),
          h(
            "pre",
            null,
            text(" export function isValidEmail(value: string) {\n"),
            text("-  return EMAIL_PATTERN.test(value);\n", "diff-remove"),
            text("+  return EMAIL_PATTERN.test(value.trim());\n", "diff-add"),
            text(" }"),
          ),
        ),
        h(
          "div",
          { class: "callout" },
          icon("shield"),
          h(
            "div",
            null,
            h(
              "strong",
              null,
              done
                ? "Sample evidence is available"
                : "The guild is still working",
            ),
            h(
              "p",
              null,
              done
                ? "Verification and review are illustrated, not executed. Download the demo log to inspect the handoffs."
                : "Advance the demo through verification and review to enable approval.",
            ),
          ),
        ),
        h(
          "div",
          { class: "dialog-actions" },
          button("Export demo log", exportLog, "", "download"),
          yes,
        ),
        h(
          "p",
          { class: "fine-print" },
          "This records a UI decision only. It never applies a patch, merges code, or accesses a repository.",
        ),
      ),
      true,
    );
  }
  function openTool(id: ToolId) {
    if (["anvil", "quench", "scanner", "acceptance-scroll"].includes(id)) {
      selectedTool = undefined;
      activityOpen = false;
      render();
      if (id === "anvil")
        scene.showAction(
          id,
          "The forge is yours.",
          "Samurai shapes the sample patch at the anvil.",
          [
            {
              label: "Send for testing →",
              run: () => {
                campaign = recordMessage(
                  campaign,
                  "samurai",
                  "sentinel",
                  "Sample patch ready: please verify whitespace handling.",
                );
                scene.deliverFinding("samurai", "sentinel");
                scene.closeAction();
                notify(
                  "Demo handoff: Samurai → Sentinel. Shared record updated.",
                );
              },
            },
            { label: "Inspect patch", run: () => openWorkbench("anvil") },
          ],
        );
      else if (id === "quench")
        scene.showAction(
          id,
          "Cool. Inspect. Refine.",
          "Quench selected. Samurai cools the workpiece before the next pass.",
          [
            {
              label: "Back to the anvil →",
              run: () => {
                scene.useTool("anvil");
                openTool("anvil");
              },
            },
          ],
        );
      else if (id === "scanner") {
        checksPreview = true;
        scene.showAction(
          id,
          "An edge case to investigate.",
          "Scripted example: surrounding whitespace fails validation. No real tests ran.",
          [
            {
              label: "Inspect finding →",
              run: () => openWorkbench("test-console"),
            },
            {
              label: "Send to Samurai",
              run: () => {
                scene.closeAction();
                act("finding");
              },
            },
          ],
        );
      } else
        scene.showAction(
          id,
          "Every criterion matters.",
          `${campaign.checked.length} of ${campaign.criteria.length} criteria marked reviewed. Open the plan to review and stamp a criterion.`,
          [
            {
              label: "Review criteria →",
              run: () => openWorkbench("plan-board"),
            },
          ],
        );
      return;
    }
    openWorkbench(id);
  }
  function openWorkbench(id: ToolId) {
    scene.closeAction();
    selectedTool = id;
    activityOpen = false;
    render();
    root
      .querySelector<HTMLSelectElement>(".workbench-panel select")
      ?.focus({ preventScroll: true });
  }
  function closeWorkbench() {
    const id = selectedTool;
    selectedTool = undefined;
    render();
    if (id)
      root
        .querySelector<SVGElement>(`[data-tool="${id}"]`)
        ?.focus({ preventScroll: true });
  }
  function act(action: CampaignAction, sender: Role = "sentinel") {
    stop();
    const result = campaignDecision(action, mission, campaign, sender);
    mission = result.mission;
    campaign = result.campaign;
    if (result.delivered) scene.deliverFinding(sender, "samurai");
    render();
    notify(campaign.decision);
  }
  function toolPanel() {
    if (!selectedTool) return null;
    const tool = guildTools.find((t) => t.id === selectedTool)!;
    const body = h(
      "div",
      { class: "workbench-body" },
      h(
        "select",
        {
          attrs: { "aria-label": "Choose a workspace tool" },
          on: {
            change: (event: Event) => {
              const id = (event.target as HTMLSelectElement).value as ToolId;
              scene.useTool(id);
              openTool(id);
            },
          },
        },
        ...guildTools.map((t) =>
          h(
            "option",
            { attrs: { value: t.id, selected: t.id === tool.id } },
            t.name,
          ),
        ),
      ),
    );
    if (tool.workspace === "campaign")
      body.append(
        button(
          "Open campaign dashboard",
          () => navigate("campaign"),
          "tool-primary",
          "expand",
        ),
      );
    const run = button(
      "Use this tool",
      () => {
        scene.useTool(tool.id);
        notify(
          `${roles.find((r) => r.id === tool.role)!.name} is heading to ${tool.name.toLowerCase()}.`,
        );
      },
      "tool-primary",
      "play",
    );
    const showRecord = () =>
      h(
        "div",
        { class: "shared-record" },
        ...campaign.messages
          .slice(-8)
          .reverse()
          .map((m) =>
            h(
              "article",
              null,
              h("strong", null, `${m.from} → ${m.to}`),
              h("p", null, m.text),
            ),
          ),
      );
    if (tool.id === "plan-board" || tool.id === "acceptance-scroll") {
      body.append(
        h("h3", null, "The acceptance plan"),
        h(
          "p",
          null,
          "Pin criteria, then mark the ones you have reviewed. These are your demo notes.",
        ),
      );
      for (const [i, criterion] of campaign.criteria.entries()) {
        const check = h("input", {
          attrs: { type: "checkbox", checked: campaign.checked.includes(i) },
          on: {
            change: () => {
              campaign.checked = campaign.checked.includes(i)
                ? campaign.checked.filter((n) => n !== i)
                : [...campaign.checked, i];
              scene.useTool("acceptance-scroll");
            },
          },
        });
        body.append(h("label", { class: "criterion-item" }, check, criterion));
      }
      const note = h("input", {
        attrs: {
          placeholder: "Add an acceptance criterion",
          maxlength: 120,
          "aria-label": "New acceptance criterion",
        },
      });
      body.append(
        h(
          "form",
          {
            class: "tool-form",
            on: {
              submit: (event: Event) => {
                event.preventDefault();
                if (note.value.trim()) {
                  campaign.criteria.push(note.value.trim());
                  scene.useTool("plan-board");
                  render();
                }
              },
            },
          },
          note,
          h(
            "button",
            { class: "btn", attrs: { type: "submit" } },
            "Pin criterion",
          ),
        ),
        run,
      );
    } else if (tool.id === "anvil" || tool.id === "quench") {
      body.append(
        h(
          "h3",
          null,
          tool.id === "anvil"
            ? "Work the sample patch"
            : "Cool, inspect, refine",
        ),
        h(
          "p",
          null,
          tool.id === "anvil"
            ? "Samurai swings the hammer over the anvil, then carries the workpiece to the barrel."
            : "Send the blacksmith to the quench barrel to cool the workpiece.",
        ),
        h(
          "pre",
          { class: "tool-code" },
          "// Illustrative patch\n- validate(value)\n+ validate(value.trim())",
        ),
        button(
          tool.id === "anvil" ? "Work at the anvil" : "Quench the workpiece",
          () => {
            scene.useTool(tool.id);
            notify("Workbench animation selected. This is a sample patch.");
          },
          "tool-primary",
          "play",
        ),
        button(
          "Send patch to Sentinel",
          () => {
            campaign = recordMessage(
              campaign,
              "samurai",
              "sentinel",
              "Sample patch ready: please verify whitespace handling.",
            );
            scene.deliverFinding("samurai", "sentinel");
            render();
          },
          "",
          "arrow",
        ),
      );
    } else if (tool.id === "test-console" || tool.id === "scanner") {
      body.append(
        h("h3", null, "Sentinel’s inspection"),
        h(
          "p",
          null,
          "Preview a scripted test result, inspect the edge case, and send the finding to Samurai.",
        ),
        badge("SCRIPTED CHECK PREVIEW", "amber"),
      );
      if (checksPreview)
        for (const [name, result] of [
          ["valid address", "PASS"],
          ["malformed input", "PASS"],
          ["surrounding whitespace", "FAIL"],
        ])
          body.append(
            h(
              "div",
              { class: "sample-check" },
              text(name!),
              badge(result!, result === "PASS" ? "green" : "red"),
            ),
          );
      body.append(
        button(
          "Preview sample checks",
          () => {
            checksPreview = true;
            scene.useTool(tool.id);
            render();
          },
          "tool-primary",
          "play",
        ),
      );
      if (checksPreview)
        body.append(
          button("Send failure to Samurai", () => act("finding"), "", "arrow"),
        );
    } else if (tool.id === "review-desk" || tool.id === "evidence-shelf") {
      body.append(
        h(
          "h3",
          null,
          tool.id === "review-desk"
            ? "Read, critique, annotate"
            : "Shared evidence archive",
        ),
      );
      if (tool.id === "review-desk") {
        body.append(
          h(
            "pre",
            { class: "tool-code" },
            "// Sample review\nInput is validated before trimming.\nRequest: normalize first.",
          ),
        );
        const note = h("textarea", {
          attrs: {
            rows: 3,
            placeholder: "Leave a review note…",
            maxlength: 400,
            "aria-label": "Review note",
          },
        });
        body.append(
          h(
            "form",
            {
              class: "tool-form",
              on: {
                submit: (event: Event) => {
                  event.preventDefault();
                  if (note.value.trim()) {
                    campaign = recordMessage(
                      campaign,
                      "shadow",
                      "samurai",
                      note.value.trim(),
                    );
                    scene.useTool("review-desk");
                    render();
                  }
                },
              },
            },
            note,
            h(
              "button",
              { class: "btn", attrs: { type: "submit" } },
              "Save review note",
            ),
          ),
          button(
            "Request a revision",
            () => act("finding", "shadow"),
            "tool-primary",
            "arrow",
          ),
        );
      }
      body.append(showRecord(), run);
    } else {
      body.append(
        h("h3", null, "The Living Campaign"),
        h(
          "p",
          null,
          "Shared findings drive the next decision. Inspect the relay, revise the plan, and decide when to stop.",
        ),
        h(
          "div",
          { class: "relay-flow" },
          h("strong", null, "Shared mission record"),
          ...[
            ["Shogun", "plan", "Samurai"],
            ["Sentinel", "failure", "Samurai"],
            ["Shadow", "critique", "Samurai"],
            ["Samurai", "repair", "Sentinel"],
          ].map(([from, reason, to]) =>
            h(
              "span",
              null,
              text(from!),
              text(`— ${reason} →`, "relay-arrow"),
              text(to!),
            ),
          ),
        ),
        h(
          "div",
          {
            class: `campaign-decision ${campaign.hold ? "held" : ""}`,
            attrs: { role: "status" },
          },
          text(
            campaign.hold ? "HUMAN INPUT REQUIRED" : "CURRENT DECISION",
            "eyebrow",
          ),
          h("p", null, campaign.decision),
        ),
        h(
          "div",
          { class: "campaign-budget" },
          text(`Iteration ${mission.iteration} of ${mission.limit}`),
          text(campaign.hold ?? "Ready", "badge"),
        ),
      );
      if (tool.id === "relay-board")
        body.append(
          button(
            "Send sample failure → Samurai",
            () => act("finding"),
            "tool-primary",
            "arrow",
          ),
          showRecord(),
        );
      else {
        const limit = h(
          "select",
          {
            attrs: { "aria-label": "Campaign iteration limit" },
            on: {
              change: (event: Event) => {
                mission.limit = Number(
                  (event.target as HTMLSelectElement).value,
                );
                render();
              },
            },
          },
          ...[1, 2, 3, 4, 5].map((n) =>
            h(
              "option",
              { attrs: { value: n, selected: n === mission.limit } },
              `${n} iterations`,
            ),
          ),
        );
        body.append(
          h("label", { class: "budget-setting" }, "Iteration budget", limit),
          h(
            "div",
            { class: "autonomy-actions" },
            button("Continue", () => act("continue"), "", "play"),
            button("Retry repair", () => act("retry"), "", "branch"),
            button("Ask for clarification", () => act("clarify"), "", "users"),
            button(
              "Stop campaign",
              () => act("stop"),
              "stop-campaign",
              "pause",
            ),
          ),
          button(
            "Simulate unavailable tool",
            () => act("tool-failure"),
            "text-btn",
            "settings",
          ),
        );
        if (campaign.hold === "clarification") {
          const response = h("textarea", {
            attrs: {
              rows: 3,
              "aria-label": "Commander clarification",
              placeholder: "Record your clarification…",
            },
            on: {
              input: (event: Event) => {
                campaign.clarification = (
                  event.target as HTMLTextAreaElement
                ).value;
              },
            },
          });
          response.value = campaign.clarification;
          body.append(
            response,
            button(
              "Record clarification & continue",
              () => act("continue"),
              "tool-primary",
              "check",
            ),
          );
        }
        body.append(
          h(
            "div",
            { class: "tool-boundaries" },
            h("strong", null, "Execution boundaries"),
            h(
              "p",
              null,
              "Success: criteria reviewed, checks verified, review resolved, human approval recorded.",
            ),
            h(
              "p",
              null,
              "UI tools only. No repository commands, file writes, or merges. Repeated tool failures pause for a human decision.",
            ),
          ),
        );
      }
    }
    return h(
      "aside",
      {
        class: "workbench-panel",
        attrs: {
          "aria-label": `${tool.name} workspace`,
          "data-workspace": tool.workspace,
        },
      },
      h(
        "div",
        { class: "workbench-heading" },
        h(
          "div",
          null,
          text(
            tool.workspace === "campaign"
              ? "COORDINATION & AUTONOMY"
              : `${tool.role.toUpperCase()} WORKSPACE`,
            "eyebrow",
          ),
          h("h2", null, tool.name),
        ),
        button("Close workspace", closeWorkbench, "close-workbench", "close"),
      ),
      body,
      h(
        "div",
        { class: "workbench-disclosure" },
        "Interactive UI simulation · no connected agent engine",
      ),
    );
  }

  function world() {
    return h(
      "section",
      {
        class: "world-panel",
        attrs: { "aria-label": "Interactive guild grounds" },
      },
      h("div", { class: `world-map ${day ? "day" : ""}` }, scene.element),
      workspaceNavigator(),
      scene.navigation.controls,
      h(
        "div",
        { class: "scene-footnote" },
        text(
          day
            ? "Daylight · still scenery"
            : motionPaused || campaign.hold
              ? "Scenery paused"
              : "Night shift · the guild is at work",
        ),
        text("UI DEMO · NO CONNECTED AGENTS"),
      ),
    );
  }
  function workspaceNavigator() {
    const items: [Workspace, string, string][] = [
      ["shogun", "Council", "Plan & delegate"],
      ["samurai", "Forge", "Build & repair"],
      ["sentinel", "Dojo", "Test & verify"],
      ["shadow", "Archive", "Review & protect"],
      ["campaign", "Campaign", "Coordinate & adapt"],
    ];
    return h(
      "nav",
      {
        class: "workspace-nav",
        attrs: { "aria-label": "Focus a guild workspace" },
      },
      h("span", { class: "nav-eyebrow" }, "EXPLORE THE GUILD"),
      ...items.map(([id, name, description], i) => {
        const b = h(
          "button",
          {
            attrs: {
              type: "button",
              "aria-label": `Focus ${name}`,
              "data-workspace": id,
              "aria-pressed": String(id === focusedWorkspace),
            },
            on: {
              click: () => {
                focusedWorkspace = id;
                scene.navigation.focusWorkspace(id);
                for (const b of root.querySelectorAll(".workspace-nav button"))
                  b.setAttribute(
                    "aria-pressed",
                    String(b.getAttribute("data-workspace") === id),
                  );
              },
            },
          },
          text(`0${i + 1}`, "workspace-number"),
          h("span", null, h("strong", null, name), text(description)),
          text("↗", "workspace-arrow"),
        );
        return b;
      }),
      h("span", { class: "nav-hint" }, "Drag to explore · scroll to zoom"),
    );
  }
  function cockpitHeader() {
    const missionButton = h(
      "button",
      { class: "mission-pill", on: { click: () => navigate("missions") } },
      text("", "status-dot"),
      h("span", null, h("strong", null, mission.title), text(mission.repo)),
      badge(campaign.hold ? "On hold" : phaseLabels[mission.phase], "amber"),
      icon("arrow"),
    );
    return h(
      "header",
      { class: "cockpit-header" },
      h(
        "a",
        {
          class: "cockpit-brand",
          attrs: { href: "#overview", "aria-label": "Maestro command center" },
          on: {
            click: (event: Event) => {
              event.preventDefault();
              navigate("overview");
            },
          },
        },
        h("span", { class: "guild-emblem" }, "M"),
        h(
          "span",
          { class: "brand-type" },
          h("strong", null, "Maestro"),
          text("THE ENGINEERING GUILD"),
        ),
      ),
      missionButton,
      h(
        "div",
        { class: "crew-summary" },
        h(
          "span",
          { class: "demo-indicator" },
          text("", "status-dot"),
          "Interactive demo",
        ),
        text("·"),
        text(`Iteration ${mission.iteration}/${mission.limit}`),
      ),
    );
  }
  function dock() {
    const next = button("Next step", step, "dock-next", "arrow");
    next.title = "Advance the sample mission";
    next.disabled =
      mission.phase === "approval" ||
      mission.phase === "complete" ||
      !!campaign.hold;
    const motion = button(
      "",
      () => {
        motionPaused = !motionPaused;
        render();
      },
      "dock-icon",
      motionPaused ? "play" : "pause",
    );
    motion.setAttribute(
      "aria-label",
      motionPaused ? "Resume scenery motion" : "Pause scenery motion",
    );
    motion.title = day
      ? "Animation is available in dark mode"
      : "Pause or resume scenery";
    motion.disabled = day || reducedMotion;
    const theme = button("", toggleTheme, "dock-icon", day ? "moon" : "sun");
    theme.setAttribute("aria-label", day ? "Dark mode" : "Light mode");
    theme.title = day ? "Switch to dark mode" : "Switch to light mode";
    const feed = button(
      "Activity",
      () => {
        activityOpen = !activityOpen;
        if (activityOpen) selectedTool = undefined;
        render();
      },
      `dock-tab ${activityOpen ? "selected" : ""}`,
      "file",
    );
    feed.setAttribute("aria-expanded", String(activityOpen));
    const full = button(
      "",
      () => {
        if (!document.fullscreenElement)
          void root
            .requestFullscreen?.()
            .catch(() => notify("Fullscreen is unavailable."));
        else void document.exitFullscreen();
      },
      "dock-icon",
      "expand",
    );
    full.setAttribute("aria-label", "Toggle fullscreen");
    full.title = "Fullscreen";
    return h(
      "nav",
      { class: "guild-dock", attrs: { "aria-label": "Guild controls" } },
      button("New mission", newMission, "dock-create", "plus"),
      h("span", { class: "dock-divider" }),
      button("Mission", () => navigate("missions"), "dock-tab", "flag"),
      button("Guild", () => navigate("guild"), "dock-tab", "users"),
      button(
        "Campaign",
        () => {
          scene.useTool("relay-board");
          openTool("relay-board");
        },
        "dock-tab campaign-tab",
        "branch",
      ),
      feed,
      h("span", { class: "dock-divider" }),
      button(
        campaign.hold
          ? "Resolve hold"
          : mission.running
            ? "Pause demo"
            : mission.phase === "approval"
              ? "Review change"
              : mission.phase === "complete"
                ? "Replay demo"
                : "Play demo",
        play,
        "dock-play",
        mission.running ? "pause" : "play",
      ),
      next,
      theme,
      motion,
      full,
      button(
        "Settings",
        () => navigate("settings"),
        "dock-settings",
        "settings",
      ),
    );
  }
  function activity() {
    const visible = updates
      .slice(0, phases.indexOf(mission.phase) + 1)
      .slice(-4)
      .reverse();
    return h(
      "aside",
      { class: "activity-panel" },
      h(
        "div",
        { class: "panel-heading" },
        h("div", null, h("h3", null, "Activity"), text("Mission updates")),
        button(
          "Close activity",
          () => {
            activityOpen = false;
            render();
          },
          "close-activity",
          "close",
        ),
      ),
      h(
        "div",
        {
          class: "activity-feed",
          attrs: { "aria-label": "Simulated agent messages" },
        },
        ...visible.map((u, i) =>
          h(
            "article",
            { class: `activity-item ${u.tone ?? ""}` },
            h("div", { class: "activity-avatar" }, avatar(u.role)),
            h(
              "div",
              { class: "activity-copy" },
              h(
                "div",
                { class: "activity-meta" },
                h("strong", null, roles.find((r) => r.id === u.role)!.name),
                text(i === 0 ? "just now" : `${i + 1}m ago`),
              ),
              h("h4", null, u.title),
              h("p", null, u.detail),
              u.tone === "failure" && badge("REPAIR REQUESTED", "red"),
              u.tone === "repair" && badge("PLAN ADAPTED", "amber"),
            ),
          ),
        ),
      ),
      h(
        "div",
        { class: "handoff-note" },
        icon("branch"),
        h(
          "div",
          null,
          h("strong", null, "Every handoff has a record"),
          h(
            "p",
            null,
            "Findings travel between agents. Every handoff leaves a trail.",
          ),
        ),
      ),
      button(
        "Open mission log",
        () => navigate("evidence"),
        "feed-link",
        "arrow",
      ),
    );
  }
  function render() {
    document.documentElement.dataset.theme = day ? "light" : "dark";
    root.className = `maestro cockpit ${view === "overview" ? "is-overview" : "is-detail"}`;
    const active = document.activeElement;
    const focusName =
      active instanceof HTMLButtonElement && root.contains(active)
        ? active.getAttribute("aria-label") || active.textContent
        : null;
    const content = h("main", {
      class: "main-content",
      attrs: { id: "main-content" },
    });
    root.classList.toggle("desk", view !== "overview");
    root.dataset.uiMotion =
      !day && !motionPaused && !reducedMotion && !campaign.hold ? "on" : "off";
    root.dataset.pageEnter = String(lastRenderedView !== view);
    lastRenderedView = view;
    const skip = h(
      "a",
      { class: "skip-link", attrs: { href: "#main-content" } },
      "Skip to content",
    );
    if (view === "overview") {
      content.append(
        h(
          "div",
          { class: `cockpit-stage ${activityOpen ? "with-activity" : ""}` },
          world(),
          activityOpen && activity(),
          toolPanel(),
        ),
      );
      root.replaceChildren(
        skip,
        cockpitHeader(),
        h("div", { class: "main-shell" }, content),
        dock(),
      );
    } else {
      const context: DetailContext = {
        view,
        mission,
        campaign,
        day,
        motionPaused,
        reducedMotion,
        filter: evidenceFilter,
        navigate,
        act,
        newMission,
        review,
        exportLog,
        agentDetails,
        reset,
        toggleTheme,
        setMotion: (enabled) => {
          motionPaused = !enabled;
          render();
        },
        setLimit: (limit) => {
          mission.limit = limit;
          render();
        },
        setClarification: (value) => {
          campaign.clarification = value;
        },
        setFilter: (filter) => {
          evidenceFilter = filter;
          render();
        },
      };
      content.append(detailPage(context));
      root.replaceChildren(
        skip,
        detailSidebar(context),
        h(
          "div",
          { class: "desk-shell" },
          detailTopbar(context),
          content,
          h(
            "footer",
            { class: "desk-footer" },
            text("Maestro / Engineering Guild"),
            text("UI preview · sample mission data"),
          ),
        ),
      );
    }
    scene.update({
      dark: !day,
      paused: motionPaused || !!campaign.hold,
      reduced: reducedMotion,
      phase: mission.phase,
      held: !!campaign.hold,
    });
    if (focusName)
      [...root.querySelectorAll("button")]
        .find(
          (b) => (b.getAttribute("aria-label") || b.textContent) === focusName,
        )
        ?.focus({ preventScroll: true });
  }
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && selectedTool && !dialog?.open) {
      closeWorkbench();
      return;
    }
    if (
      e.key.toLowerCase() === "n" &&
      !dialog?.open &&
      !(e.target instanceof HTMLInputElement) &&
      !(e.target instanceof HTMLTextAreaElement) &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey
    ) {
      e.preventDefault();
      newMission();
    }
  });
  window.addEventListener("pagehide", () => {
    stop();
    scene.dispose();
  });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) render();
  });
  render();
}
