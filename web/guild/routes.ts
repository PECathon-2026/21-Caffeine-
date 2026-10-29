import type { Role } from "./mission.ts";
import {
  entrances,
  guildTools,
  inWorkspace,
  meeting,
  type Bounds,
  type Point,
  type Workspace,
} from "./layout.ts";

export interface WalkEdge {
  from: string;
  to: string;
  outdoor: boolean;
}
export const walkNodes: Record<string, Point> = {
  north: { x: 580, y: 286 },
  south: { x: 580, y: 560 },
  court: meeting,
  "court-west": { x: 552, y: 420 },
  "court-east": { x: 608, y: 420 },
  "archive-road": { x: 212, y: 286 },
  "campaign-road": { x: 970, y: 286 },
  "forge-road": { x: 316, y: 560 },
  "dojo-road": { x: 895, y: 560 },
  "shogun-door": entrances.shogun,
  "samurai-door": entrances.samurai,
  "sentinel-door": entrances.sentinel,
  "shadow-door": entrances.shadow,
  "campaign-door": { x: 970, y: 242 },
};
function inside(id: string, workspace: Workspace, x: number, y: number) {
  walkNodes[id] = inWorkspace(workspace, { x, y });
}
inside("council-aisle", "shogun", 450, 250);
inside("council-scroll-aisle", "shogun", 503, 250);
inside("forge-aisle", "samurai", 246, 388);
inside("forge-barrel-aisle", "samurai", 286, 388);
inside("dojo-aisle", "sentinel", 665, 388);
inside("dojo-scanner-aisle", "sentinel", 735, 388);
inside("archive-aisle", "shadow", 192, 575);
inside("archive-side", "shadow", 142, 575);
inside("archive-shelf-side", "shadow", 142, 522);
inside("campaign-aisle", "campaign", 680, 592);
inside("campaign-board-aisle", "campaign", 589, 592);
for (const tool of guildTools) walkNodes[tool.id] = tool.stand;
export const walkEdges: WalkEdge[] = [];
function connect(outdoor: boolean, ...nodes: string[]) {
  for (let i = 1; i < nodes.length; i++)
    walkEdges.push({ from: nodes[i - 1]!, to: nodes[i]!, outdoor });
}
connect(true, "archive-road", "north", "campaign-road");
connect(true, "north", "court", "south");
connect(true, "forge-road", "south", "dojo-road");
connect(true, "archive-road", "shadow-door");
connect(true, "north", "shogun-door");
connect(true, "campaign-road", "campaign-door");
connect(true, "forge-road", "samurai-door");
connect(true, "dojo-road", "sentinel-door");
connect(false, "court-west", "court", "court-east");
connect(false, "shogun-door", "council-aisle", "plan-board");
connect(false, "council-aisle", "council-scroll-aisle", "acceptance-scroll");
connect(false, "samurai-door", "forge-aisle", "anvil");
connect(false, "forge-aisle", "forge-barrel-aisle", "quench");
connect(false, "sentinel-door", "dojo-aisle", "test-console");
connect(false, "dojo-aisle", "dojo-scanner-aisle", "scanner");
connect(false, "shadow-door", "archive-aisle", "review-desk");
connect(
  false,
  "archive-aisle",
  "archive-side",
  "archive-shelf-side",
  "evidence-shelf",
);
connect(
  false,
  "campaign-door",
  "campaign-aisle",
  "campaign-board-aisle",
  "relay-board",
);
connect(false, "campaign-aisle", "autonomy-console");

const distance = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y);
export function projectOnEdge(p: Point, a: Point, b: Point): Point {
  const dx = b.x - a.x,
    dy = b.y - a.y,
    length = dx * dx + dy * dy;
  const t = length
    ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / length))
    : 0;
  return { x: a.x + t * dx, y: a.y + t * dy };
}
export function onWalkway(p: Point, tolerance = 0.01) {
  return walkEdges.some(
    (e) =>
      distance(p, projectOnEdge(p, walkNodes[e.from]!, walkNodes[e.to]!)) <=
      tolerance,
  );
}
/** Replan from the actual point on the current edge, not from the old destination's room. */
export function routeTo(from: Point, destination: string): Point[] | null {
  if (!walkNodes[destination]) return null;
  const costs = new Map<string, number>([[destination, 0]]),
    next = new Map<string, string>(),
    visited = new Set<string>();
  while (true) {
    const current = [...costs.entries()]
      .filter(([id]) => !visited.has(id))
      .sort((a, b) => a[1] - b[1])[0];
    if (!current) break;
    const [id, cost] = current;
    visited.add(id);
    for (const e of walkEdges) {
      const other = e.from === id ? e.to : e.to === id ? e.from : undefined;
      if (!other) continue;
      const candidate = cost + distance(walkNodes[id]!, walkNodes[other]!);
      if (candidate < (costs.get(other) ?? Infinity)) {
        costs.set(other, candidate);
        next.set(other, id);
      }
    }
  }
  let best: { cost: number; start: string; point: Point } | undefined;
  for (const e of walkEdges) {
    const point = projectOnEdge(from, walkNodes[e.from]!, walkNodes[e.to]!);
    if (distance(point, from) > 0.01) continue; // An invalid origin stops; it never cuts across scenery.
    for (const id of [e.from, e.to]) {
      const cost =
        distance(point, walkNodes[id]!) + (costs.get(id) ?? Infinity);
      if (!best || cost < best.cost) best = { cost, start: id, point };
    }
  }
  if (!best || !Number.isFinite(best.cost)) return null;
  const points: Point[] = [best.point];
  let id = best.start;
  for (let i = 0; i <= Object.keys(walkNodes).length; i++) {
    points.push(walkNodes[id]!);
    if (id === destination) break;
    const following = next.get(id);
    if (!following) return null;
    id = following;
  }
  return points
    .filter((p, i) => distance(p, i ? points[i - 1]! : from) > 0.001)
    .map((p) => ({ ...p }));
}

export const handoffNodes: Record<"sender" | "receiver", string> = {
  sender: "court-west",
  receiver: "court-east",
};
/** Ground footprints, independent of artwork height: actors can walk in front of tall objects. */
export const solidFootprints: (Bounds & {
  id: string;
  workspace: Workspace;
})[] = [];
function solid(
  workspace: Workspace,
  id: string,
  x: number,
  y: number,
  width: number,
  height: number,
) {
  solidFootprints.push({
    ...inWorkspace(workspace, { x, y }),
    width,
    height,
    id,
    workspace,
  });
}
solid("shogun", "council-table", 395, 218, 120, 13);
solid("shogun", "pagoda", 375, 70, 150, 128);
solid("samurai", "forge-wall", 85, 245, 219, 68);
solid("samurai", "furnace", 101, 312, 71, 48);
solid("samurai", "anvil", 185, 338, 53, 14);
solid("samurai", "barrel", 251, 357, 27, 24);
solid("samurai", "right-post", 295, 255, 7, 139);
solid("samurai", "supply-crate", 162, 374, 20, 17);
solid("sentinel", "dojo-wall", 600, 254, 204, 45);
solid("sentinel", "test-monitors", 623, 326, 93, 14);
solid("sentinel", "scanner", 747, 346, 45, 14);
solid("shadow", "bookshelves", 113, 484, 179, 28);
solid("shadow", "review-table", 163, 539, 96, 17);
solid("campaign", "mission-board", 535, 484, 115, 62);
solid("campaign", "relay-table", 548, 553, 123, 23);
solid("campaign", "console", 689, 516, 106, 53);
solid("campaign", "seal-bench", 688, 578, 111, 7);
export function clearFootprint(point: Point, radius = 4) {
  return !solidFootprints.some(
    (b) =>
      point.x > b.x - radius &&
      point.x < b.x + b.width + radius &&
      point.y > b.y - radius &&
      point.y < b.y + b.height + radius,
  );
}
export function routeLabel(role: Role) {
  return `${role}-door`;
}
