import { h, svgEl } from "../ui/dom.ts";
import { guildLayers } from "./world.ts";
import { materials } from "./materials.ts";
import { routeTo, walkNodes, handoffNodes } from "./routes.ts";
import { createNavigation } from "./navigation.ts";
import { workingStation } from "./follow-camera.ts";
import {
  characterMarkup,
  animationProfiles,
  idleThoughts,
  intentions,
  motionAllowed,
  moveToward,
  type Pose,
  type Direction,
  directionToward,
} from "./characters.ts";
import {
  WORLD,
  entrances,
  guildTools,
  homeTools,
  roleOrder,
  workspaceOffsets,
  type Point,
  type ToolId,
} from "./layout.ts";
import { phaseOwner, roles, type Phase, type Role } from "./mission.ts";

interface Actor {
  role: Role;
  position: Point;
  route: Point[];
  pose: Pose;
  elapsed: number;
  tool: ToolId;
  facing: Direction;
  turnRemaining: number;
  returnHome: boolean;
  goalNode: string;
  routeBlocked: boolean;
}
const toolById = (id: ToolId) => guildTools.find((t) => t.id === id)!;
export function createActor(role: Role, index: number): Actor {
  return {
    role,
    position: { ...entrances[role] },
    route: routeTo(entrances[role], homeTools[role]) ?? [],
    pose: "walking",
    elapsed: -index * 0.6,
    tool: homeTools[role],
    facing: "south",
    turnRemaining: 0,
    returnHome: false,
    goalNode: homeTools[role],
    routeBlocked: false,
  };
}
/** Commands can interrupt any edge without changing the actor's actual position. */
export function directActor(
  actor: Actor,
  id: ToolId,
  goalNode: string = id,
  instant = false,
) {
  const route = routeTo(actor.position, goalNode);
  actor.routeBlocked = route === null;
  if (!route) {
    actor.route = [];
    actor.pose = "idle";
    return false;
  }
  actor.tool = id;
  actor.goalNode = goalNode;
  actor.route = route;
  actor.elapsed = 0;
  actor.turnRemaining = 0;
  actor.pose = route.length ? "walking" : "thinking";
  if (instant) {
    settleActor(actor);
  }
  return true;
}
/** Still scenery completes travel at its requested tool, with a purposeful pose. */
export function settleActor(actor: Actor) {
  const goal = walkNodes[actor.goalNode];
  if (!goal || actor.routeBlocked) return;
  actor.position = { ...goal };
  actor.route = [];
  actor.elapsed = 2;
  actor.turnRemaining = 0;
  actor.pose = "working";
  actor.facing = toolFacing(actor.tool);
}
function toolFacing(tool: ToolId): Direction {
  return tool === "evidence-shelf"
    ? "north"
    : tool === "scanner"
      ? "east"
      : "south";
}
export function atDestination(actor: Actor) {
  const goal = walkNodes[actor.goalNode];
  return (
    !actor.routeBlocked &&
    !actor.route.length &&
    !!goal &&
    Math.hypot(actor.position.x - goal.x, actor.position.y - goal.y) < 0.01
  );
}
export function tickActor(actor: Actor, seconds: number) {
  actor.elapsed += seconds;
  if (actor.elapsed < 0) return;
  if (actor.turnRemaining > 0) {
    actor.turnRemaining -= seconds;
    actor.pose = "idle";
    return;
  }
  const target = actor.route[0];
  if (target) {
    const direction = directionToward(actor.position, target);
    if (
      Math.hypot(target.x - actor.position.x, target.y - actor.position.y) >
        0.2 &&
      direction !== actor.facing
    ) {
      actor.facing = direction;
      actor.turnRemaining = 0.16;
      actor.pose = "idle";
      return;
    }
    actor.pose = "walking";
    actor.position = moveToward(
      actor.position,
      target,
      seconds * animationProfiles[actor.role].speed,
    );
    if (actor.position.x === target.x && actor.position.y === target.y) {
      actor.route.shift();
      actor.turnRemaining = actor.route.length ? 0.12 : 0;
      if (!actor.route.length) {
        actor.elapsed = 0;
        actor.pose = "thinking";
      }
    }
  } else {
    actor.pose = actor.elapsed < 1.6 ? "thinking" : "working";
    actor.facing = toolFacing(actor.tool);
  }
}
const alternate: Record<Role, ToolId> = {
  shogun: "acceptance-scroll",
  samurai: "quench",
  sentinel: "scanner",
  shadow: "evidence-shelf",
};
const dwell: Record<Role, number> = {
  shogun: 14,
  samurai: 9,
  sentinel: 11,
  shadow: 17,
};
const actionText: Record<ToolId, string> = {
  "plan-board": "Mapping the next move",
  "acceptance-scroll": "Stamping a criterion",
  anvil: "Shaping the patch",
  quench: "Cooling the workpiece",
  "test-console": "Checking the cases",
  scanner: "Scanning the boundary",
  "review-desk": "Reading and annotating",
  "evidence-shelf": "Following the evidence",
  "relay-board": "Sharing a finding",
  "autonomy-console": "Checking the boundaries",
};
export function createGuildScene(
  onInspect: (role: Role) => void,
  onTool: (id: ToolId) => void,
) {
  const element = document.createElement("div");
  element.className = "world-art";
  const actors = roleOrder.map(createActor);
  let action: { id: ToolId; element: HTMLElement } | undefined;
  function positionAction() {
    if (!action) return;
    const box = navigation.box(),
      point = toolById(action.id).anchor;
    const width = element.clientWidth || 900,
      height = element.clientHeight || 650;
    const left = ((point.x - box.x) / box.width) * width;
    const top = ((point.y - box.y) / box.height) * height;
    action.element.style.left = `${Math.max(12, Math.min(width - 280, left - 128))}px`;
    action.element.style.top = `${Math.max(12, Math.min(height - 170, top + 42))}px`;
  }
  const navigation = createNavigation(element, positionAction);
  function closeAction(restoreFocus = false) {
    const id = action?.id;
    action?.element.remove();
    action = undefined;
    if (restoreFocus && id)
      element
        .querySelector<SVGElement>(`[data-tool="${id}"]`)
        ?.focus({ preventScroll: true });
  }
  function showAction(
    id: ToolId,
    title: string,
    detail: string,
    actions: { label: string; run: () => void }[],
  ) {
    closeAction();
    const panel = h(
      "div",
      {
        class: "scene-action",
        attrs: {
          role: "dialog",
          "aria-label": title,
          "data-workspace": toolById(id).workspace,
        },
      },
      h("span", { class: "action-kicker" }, "WORKSPACE INTERACTION · DEMO"),
      h("strong", null, title),
      h("p", null, detail),
    );
    const close = h(
      "button",
      {
        class: "action-close",
        attrs: { type: "button", "aria-label": "Close tool action" },
        on: { click: () => closeAction(true) },
      },
      "×",
    );
    panel.append(close);
    const buttons = h("div", { class: "action-buttons" });
    for (const item of actions)
      buttons.append(
        h(
          "button",
          { attrs: { type: "button" }, on: { click: item.run } },
          item.label,
        ),
      );
    panel.append(buttons);
    element.append(panel);
    action = { id, element: panel };
    positionAction();
    (buttons.querySelector("button") ?? close).focus({ preventScroll: true });
  }
  element.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && action) {
      event.stopPropagation();
      closeAction(true);
    }
  });
  let dark = true,
    paused = false,
    reduced = false,
    phase: Phase = "building",
    held = false,
    engagedRole: Role | undefined,
    selected: ToolId | undefined;
  let previous = 0,
    clock = 0,
    paintAt = 0,
    frameId = 0,
    mounted = false;
  let layers: SVGGElement[] = [],
    labels: SVGGElement[] = [],
    effects: SVGGElement,
    routeLayer: SVGGElement,
    depthLayer: SVGGElement,
    props: { element: SVGGElement; y: number; id: string }[] = [],
    targets: SVGGElement[] = [],
    depthOrder = "";
  let relay: { from: Role; to: Role; age: number } | undefined;
  function refreshFollow() {
    const engaged = engagedRole
      ? toolById(actors.find((a) => a.role === engagedRole)!.tool).workspace
      : undefined;
    navigation.followTarget(workingStation(phase, !!relay, held, engaged));
  }
  function allowed() {
    return motionAllowed(
      dark,
      paused,
      reduced,
      !document.hidden && element.isConnected,
    );
  }
  function setDestination(actor: Actor, id: ToolId, node: string = id) {
    return directActor(actor, id, node, !allowed());
  }
  function cancelRelay() {
    if (!relay) return;
    const participants = [relay.from, relay.to];
    relay = undefined;
    for (const role of participants) {
      const actor = actors.find((a) => a.role === role)!;
      actor.returnHome = false;
      setDestination(actor, homeTools[role]);
    }
  }
  function useTool(id: ToolId) {
    cancelRelay();
    selected = id;
    const a = actors.find((a) => a.role === toolById(id).role)!;
    engagedRole = a.role;
    a.returnHome = false;
    setDestination(a, id);
    paint();
  }
  function deliverFinding(from: Role, to: Role) {
    cancelRelay();
    engagedRole = to;
    relay = { from, to, age: 0 };
    for (const [i, role] of [from, to].entries()) {
      const a = actors.find((a) => a.role === role)!;
      setDestination(
        a,
        "relay-board",
        i === 0 ? handoffNodes.sender : handoffNodes.receiver,
      );
      a.returnHome = true;
    }
    selected = "relay-board";
    // A still scene must not leave a handoff waiting for an animation clock.
    if (!allowed()) cancelRelay();
    paint();
  }
  function build() {
    closeAction();
    const scenery = guildLayers(dark);
    element.innerHTML = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${WORLD.width} ${WORLD.height}" shape-rendering="crispEdges">${materials(dark)}${scenery.ground}</svg>`;
    element.querySelector("svg")!.setAttribute("aria-hidden", "true");
    const svg = svgEl("svg", {
      viewBox: `0 0 ${WORLD.width} ${WORLD.height}`,
      class: "characters-layer",
      role: "group",
      "shape-rendering": "crispEdges",
      "aria-label": "Guild characters and interactive tools",
    });
    element.append(svg);
    routeLayer = svgEl("g", { "aria-hidden": "true", class: "selected-route" });
    depthLayer = svgEl("g", { class: "depth-layer" });
    svg.append(routeLayer, depthLayer);
    props = scenery.objects.map((object) => {
      const group = svgEl("g", {
        "aria-hidden": "true",
        "data-depth-object": object.id,
        "data-depth-y": object.y,
        "pointer-events": "none",
      });
      group.innerHTML = object.markup;
      depthLayer.append(group);
      return { element: group, y: object.y, id: object.id };
    });
    depthOrder = "";
    const light = svgEl("g", {
      "aria-hidden": "true",
      "pointer-events": "none",
    });
    light.innerHTML = scenery.lights;
    svg.append(light);
    effects = svgEl("g", { "aria-hidden": "true", class: "tool-effects" });
    svg.append(effects);
    targets = guildTools.map((tool) => {
      const group = svgEl("g", {
        "data-tool": tool.id,
        role: "button",
        tabindex: 0,
        "aria-label": `Use ${tool.name}`,
        class: "tool-target",
      });
      const { x, y } = tool.anchor;
      group.innerHTML = `<rect class="tool-hit" x="${x - 21}" y="${y - 18}" width="42" height="36" rx="3"/><path class="tool-corner" d="M${x + 12} ${y - 13}h6v6"/><g class="tool-tip"><rect x="${x - 85}" y="${y - 42}" width="170" height="22" rx="2"/><text x="${x}" y="${y - 27}" text-anchor="middle">${tool.name}</text></g>`;
      group.addEventListener("click", () => {
        useTool(tool.id);
        onTool(tool.id);
      });
      group.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          useTool(tool.id);
          onTool(tool.id);
        }
      });
      svg.append(group);
      return group;
    });
    layers = actors.map((a) => {
      const group = svgEl("g", {
        "data-character": a.role,
        role: "button",
        tabindex: 0,
        "aria-label": `Meet ${roles.find((r) => r.id === a.role)!.name}`,
        class: "guild-character",
      });
      group.addEventListener("click", () => onInspect(a.role));
      group.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onInspect(a.role);
        }
      });
      depthLayer.append(group);
      return group;
    });
    labels = actors.map(() => {
      const group = svgEl("g", {
        "aria-hidden": "true",
        "pointer-events": "none",
      });
      svg.append(group);
      return group;
    });
    navigation.sync();
    paint();
  }
  function paint() {
    refreshFollow();
    const owner = phaseOwner[phase],
      fx: string[] = [];
    for (let i = 0; i < actors.length; i++) {
      const a = actors[i]!,
        group = layers[i]!,
        profile = animationProfiles[a.role];
      const f =
        !dark || reduced
          ? 0
          : Math.floor((clock + i * 0.39) * profile.rate) % profile.frames;
      const x = Math.round(a.position.x),
        y = Math.round(a.position.y);
      const atCampaign = toolById(a.tool).workspace === "campaign";
      const pose =
        phase === "approval" || phase === "complete"
          ? "idle"
          : atCampaign && a.pose === "working"
            ? "thinking"
            : a.pose;
      const message =
        phase === "approval" || phase === "complete"
          ? intentions[phase]
          : pose === "walking"
            ? "On my way…"
            : pose === "thinking"
              ? idleThoughts[a.role]
              : actionText[a.tool];
      const active = a.role === owner;
      group.dataset.pose = pose;
      group.dataset.goal = a.goalNode;
      group.dataset.depthY = String(a.position.y);
      group.dataset.routeBlocked = String(a.routeBlocked);
      group.dataset.route = a.route.map((p) => `${p.x},${p.y}`).join(" ");
      group.dataset.direction = a.facing;
      group.dataset.activeTool = a.tool;
      group.dataset.frame = String(f);
      group.dataset.x = String(x);
      group.dataset.y = String(y);
      group.innerHTML = characterMarkup(
        a.role,
        x,
        y,
        f,
        pose,
        a.tool,
        a.facing,
      );
      labels[i]!.innerHTML =
        (active
          ? `<g class="character-thought" transform="translate(${x} ${y - 62})"><path d="M-75-12H75V10H5L0 15V10H-75Z" fill="#191724" stroke="#b59770"/><text text-anchor="middle" y="2" fill="#f8e1bd" font-family="Pixel,monospace" font-size="10">${message}</text></g>`
          : "") +
        `<text x="${x}" y="${y + 13}" text-anchor="middle" class="sprite-name">${roles.find((r) => r.id === a.role)!.name}</text>`;
      if (pose !== "working" || atCampaign) continue;
      const shift = workspaceOffsets[toolById(a.tool).workspace];
      fx.push(`<g transform="translate(${shift.x} ${shift.y})">`);
      if (a.tool === "anvil" && f === 4)
        fx.push(
          '<path d="m211 319-5-6m12 6 6-9m-6 15 11-2" stroke="#ffdb8c" stroke-width="2"/>',
        );
      if (a.tool === "quench")
        for (let j = 0; j < 3; j++)
          fx.push(
            `<rect x="${256 + j * 6}" y="${352 - ((f + j) % 5) * 4}" width="5" height="4" fill="#badbdc" opacity=".6"/>`,
          );
      if (a.tool === "scanner")
        fx.push(
          `<path d="M753 ${313 + f * 5}h31" stroke="#7dffe2" stroke-width="2"/><rect x="752" y="306" width="32" height="44" fill="#55e8cb" opacity=".06"/>`,
        );
      if (a.tool === "test-console")
        fx.push(
          `<rect x="${630 + (f % 3) * 34}" y="313" width="10" height="3" fill="#a6ffe4"/>`,
        );
      if (a.tool === "acceptance-scroll" && f > 2)
        fx.push('<rect x="496" y="213" width="6" height="5" fill="#d65c57"/>');
      if (a.tool === "review-desk")
        fx.push(
          `<path d="M209 535h${3 + f * 2}" stroke="#a56ac3" stroke-width="1"/>`,
        );
      if (a.tool === "evidence-shelf")
        fx.push(
          `<rect x="${162 + (f % 5) * 15}" y="482" width="8" height="12" fill="#d6b8eb" opacity=".5"/>`,
        );
      fx.push("</g>");
    }
    if (relay) {
      fx.push(
        `<path d="M558 420h${Math.min(44, Math.round(relay.age * 15))}" stroke="#f2be74" stroke-width="2"/>`,
      );
    }
    effects.innerHTML = fx.join("");
    const ordered = [
      ...props,
      ...actors.map((a, i) => ({
        element: layers[i]!,
        y: a.position.y,
        id: a.role,
      })),
    ].sort((a, b) => a.y - b.y || a.id.localeCompare(b.id));
    const order = ordered.map((p) => p.id).join("|");
    if (order !== depthOrder) {
      for (const item of ordered) depthLayer.append(item.element);
      depthOrder = order;
    }
    routeLayer.innerHTML = actors
      .filter(
        (a) =>
          a.route.length &&
          (relay
            ? [relay.from, relay.to].includes(a.role)
            : selected === a.tool),
      )
      .map(
        (a) =>
          `<path d="M${a.position.x} ${a.position.y}${a.route.map((p) => `L${p.x} ${p.y}`).join("")}" stroke="${a.role === "sentinel" ? "#86d4c0" : "#e7bd7d"}" fill="none" stroke-width="1.25" stroke-dasharray="3 5" opacity=".6"/>`,
      )
      .join("");
    for (const [i, target] of targets.entries())
      target.classList.toggle("selected", guildTools[i]!.id === selected);
  }
  function sync() {
    cancelAnimationFrame(frameId);
    frameId = 0;
    previous = 0;
    const running = allowed();
    element.dataset.motion = running ? "running" : "still";
    navigation.setMotion(running);
    refreshFollow();
    if (running) frameId = requestAnimationFrame(tick);
  }
  function tick(time: number) {
    if (!allowed()) {
      sync();
      return;
    }
    const dt = previous ? Math.min((time - previous) / 1000, 0.05) : 0;
    previous = time;
    clock += dt;
    if (relay) {
      const pair = actors.filter(
        (a) => a.role === relay!.from || a.role === relay!.to,
      );
      if (pair.every(atDestination)) relay.age += dt;
      if (relay.age > 3.2) cancelRelay();
    }
    for (const a of actors) {
      if (phase === "approval" || phase === "complete") continue;
      tickActor(a, dt);
      if (relay && (a.role === relay.from || a.role === relay.to)) continue;
      if (
        !a.routeBlocked &&
        !a.route.length &&
        a.elapsed >
          (a.returnHome ? 4 : a.tool === homeTools[a.role] ? dwell[a.role] : 6)
      ) {
        const id = a.returnHome
          ? homeTools[a.role]
          : a.tool === homeTools[a.role]
            ? alternate[a.role]
            : homeTools[a.role];
        a.returnHome = false;
        setDestination(a, id);
      }
    }
    refreshFollow();
    navigation.step(dt);
    if (time - paintAt > 85) {
      paint();
      paintAt = time;
    }
    frameId = requestAnimationFrame(tick);
  }
  build();
  document.addEventListener("visibilitychange", sync);
  return {
    element,
    useTool,
    deliverFinding,
    showAction,
    closeAction,
    navigation,
    update(options: {
      dark: boolean;
      paused: boolean;
      reduced: boolean;
      phase: Phase;
      held?: boolean;
    }) {
      document.addEventListener("visibilitychange", sync);
      const changed = dark !== options.dark,
        phaseChanged = phase !== options.phase;
      dark = options.dark;
      paused = options.paused;
      reduced = options.reduced;
      phase = options.phase;
      held = options.held ?? false;
      if (phaseChanged) engagedRole = undefined;
      if (!dark || reduced || (!mounted && paused)) {
        cancelRelay();
        for (const a of actors) settleActor(a);
      }
      mounted = true;
      if (changed) build();
      if (phaseChanged) {
        if (phase === "repairing") {
          if (!relay) deliverFinding("sentinel", "samurai");
        } else {
          cancelRelay();
          const a = actors.find((a) => a.role === phaseOwner[phase])!;
          setDestination(a, homeTools[a.role]);
        }
      }
      paint();
      sync();
    },
    dispose() {
      cancelAnimationFrame(frameId);
      navigation.dispose();
      closeAction();
      document.removeEventListener("visibilitychange", sync);
    },
  };
}
