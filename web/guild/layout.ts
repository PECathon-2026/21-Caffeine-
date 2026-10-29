import type { Role } from "./mission.ts";
export interface Point {
  x: number;
  y: number;
}
export interface Bounds extends Point {
  width: number;
  height: number;
}
export type Workspace = Role | "campaign";
export const WORLD = { width: 1160, height: 650 };
/** One translation per room keeps artwork, tools, navigation and lighting aligned. */
export const workspaceOffsets: Record<Workspace, Point> = {
  shogun: { x: 130, y: -25 },
  samurai: { x: 70, y: 115 },
  sentinel: { x: 230, y: 105 },
  shadow: { x: 20, y: -362 },
  campaign: { x: 290, y: -380 },
};
export function inWorkspace(workspace: Workspace, point: Point): Point {
  const offset = workspaceOffsets[workspace];
  return { x: point.x + offset.x, y: point.y + offset.y };
}
/** Bounds include roofs and foundations. Keep at least 24 art pixels between stations. */
export const workspaceBounds: Record<Workspace, Bounds> = {
  shogun: { x: 352, y: 50, width: 195, height: 218 },
  samurai: { x: 60, y: 212, width: 265, height: 207 },
  sentinel: { x: 575, y: 224, width: 260, height: 195 },
  shadow: { x: 100, y: 462, width: 208, height: 145 },
  campaign: { x: 510, y: 462, width: 315, height: 145 },
};
for (const workspace of Object.keys(workspaceBounds) as Workspace[]) {
  Object.assign(
    workspaceBounds[workspace],
    inWorkspace(workspace, workspaceBounds[workspace]),
  );
}
export const stations: Record<Role, Point> = {
  shogun: inWorkspace("shogun", { x: 450, y: 238 }),
  samurai: inWorkspace("samurai", { x: 246, y: 357 }),
  sentinel: inWorkspace("sentinel", { x: 665, y: 350 }),
  shadow: inWorkspace("shadow", { x: 192, y: 565 }),
};
/** Front entrances connect to visible steps, never to a wall or the rear of a room. */
export const entrances: Record<Role, Point> = {
  shogun: { x: 580, y: 266 },
  samurai: { x: 316, y: 548 },
  sentinel: { x: 895, y: 540 },
  shadow: { x: 212, y: 258 },
};
export const meeting: Point = { x: 580, y: 420 };
export const roleOrder: Role[] = ["shogun", "samurai", "sentinel", "shadow"];
export type ToolId =
  | "plan-board"
  | "acceptance-scroll"
  | "anvil"
  | "quench"
  | "test-console"
  | "scanner"
  | "review-desk"
  | "evidence-shelf"
  | "relay-board"
  | "autonomy-console";
export interface GuildTool {
  id: ToolId;
  name: string;
  role: Role;
  workspace: Workspace;
  anchor: Point;
  stand: Point;
  description: string;
}
export const guildTools: GuildTool[] = [
  {
    id: "plan-board",
    name: "Strategy table",
    role: "shogun",
    workspace: "shogun",
    anchor: { x: 444, y: 215 },
    stand: { x: 450, y: 238 },
    description: "Inspect the plan and pin an acceptance criterion.",
  },
  {
    id: "acceptance-scroll",
    name: "Acceptance scroll",
    role: "shogun",
    workspace: "shogun",
    anchor: { x: 503, y: 215 },
    stand: { x: 503, y: 241 },
    description: "Mark the criteria that the sample mission must satisfy.",
  },
  {
    id: "anvil",
    name: "Forge anvil",
    role: "samurai",
    workspace: "samurai",
    anchor: { x: 213, y: 329 },
    stand: { x: 246, y: 357 },
    description: "Work the sample patch at the anvil.",
  },
  {
    id: "quench",
    name: "Quench barrel",
    role: "samurai",
    workspace: "samurai",
    anchor: { x: 264, y: 366 },
    stand: { x: 286, y: 374 },
    description: "Send Samurai to cool the workpiece.",
  },
  {
    id: "test-console",
    name: "Test console",
    role: "sentinel",
    workspace: "sentinel",
    anchor: { x: 657, y: 319 },
    stand: { x: 665, y: 350 },
    description: "Preview the checks and send a failure back to the builder.",
  },
  {
    id: "scanner",
    name: "Inspection scanner",
    role: "sentinel",
    workspace: "sentinel",
    anchor: { x: 766, y: 333 },
    stand: { x: 735, y: 365 },
    description: "Scan the sample change for edge cases.",
  },
  {
    id: "review-desk",
    name: "Review desk",
    role: "shadow",
    workspace: "shadow",
    anchor: { x: 209, y: 540 },
    stand: { x: 192, y: 565 },
    description: "Read the diff and record a review finding.",
  },
  {
    id: "evidence-shelf",
    name: "Evidence archive",
    role: "shadow",
    workspace: "shadow",
    anchor: { x: 202, y: 492 },
    stand: { x: 196, y: 522 },
    description: "Browse the shared mission record.",
  },
  {
    id: "relay-board",
    name: "Shared mission board",
    role: "shogun",
    workspace: "campaign",
    anchor: { x: 584, y: 510 },
    stand: { x: 589, y: 584 },
    description:
      "Exchange findings and watch the receiving agent change course.",
  },
  {
    id: "autonomy-console",
    name: "Autonomy controls",
    role: "shogun",
    workspace: "campaign",
    anchor: { x: 742, y: 535 },
    stand: { x: 718, y: 592 },
    description:
      "Continue, retry, request clarification, or stop within the iteration budget.",
  },
];
for (const tool of guildTools) {
  tool.anchor = inWorkspace(tool.workspace, tool.anchor);
  tool.stand = inWorkspace(tool.workspace, tool.stand);
}
export const homeTools: Record<Role, ToolId> = {
  shogun: "plan-board",
  samurai: "anvil",
  sentinel: "test-console",
  shadow: "review-desk",
};
/** Outdoor centre lines, also drawn verbatim as the stone paths. */
export function corridor(role: Role): Point[] {
  const entrance = entrances[role];
  const roadY = role === "shogun" || role === "shadow" ? 286 : 560;
  return [entrance, { x: entrance.x, y: roadY }, { x: 580, y: roadY }, meeting];
}
export function boundsGap(a: Bounds, b: Bounds) {
  return Math.hypot(
    Math.max(a.x - b.x - b.width, b.x - a.x - a.width, 0),
    Math.max(a.y - b.y - b.height, b.y - a.y - a.height, 0),
  );
}
