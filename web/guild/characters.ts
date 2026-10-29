import type { Phase, Role } from "./mission.ts";
import type { ToolId } from "./layout.ts";
export { stations, entrances, meeting, roleOrder } from "./layout.ts";
export type { Point } from "./layout.ts";
import type { Point } from "./layout.ts";
export type Pose = "walking" | "thinking" | "working" | "idle";
export type Direction = "north" | "south" | "east" | "west";
export function directionToward(from: Point, to: Point): Direction {
  const dx = to.x - from.x,
    dy = to.y - from.y;
  return Math.abs(dx) > Math.abs(dy)
    ? dx < 0
      ? "west"
      : "east"
    : dy < 0
      ? "north"
      : "south";
}
export const intentions: Record<Phase, string> = {
  planning: "Mapping the plan…",
  building: "Forging the change…",
  testing: "Checking the cases…",
  repairing: "Rethinking the fix…",
  verifying: "Verifying the repair…",
  reviewing: "Reading the diff…",
  approval: "Waiting for your seal",
  complete: "Mission complete",
};
export const idleThoughts: Record<Role, string> = {
  shogun: "Where does this lead?",
  samurai: "Measure twice…",
  sentinel: "Checking the boundary",
  shadow: "What did we miss?",
};
export const animationProfiles = {
  shogun: { speed: 38, frames: 6, rate: 2.5, action: "unroll-and-stamp" },
  samurai: { speed: 61, frames: 6, rate: 7, action: "hammer-and-quench" },
  sentinel: { speed: 48, frames: 8, rate: 4, action: "scan-and-signal" },
  shadow: { speed: 73, frames: 8, rate: 3, action: "read-turn-annotate" },
} as const;

/** Four independent silhouettes. No shared body template or generic work-arm loop. */
export function characterPixels(
  role: Role,
  frame = 0,
  pose: Pose = "idle",
  tool?: ToolId,
  direction: Direction = "south",
): string {
  if (direction !== "south")
    return directionalPixels(role, frame, pose, direction, tool);
  const out: string[] = [];
  const r = (x: number, y: number, w: number, h: number, c: string) =>
    out.push(
      `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${c}"/>`,
    );
  const p = (d: string, c: string) => out.push(`<path d="${d}" fill="${c}"/>`);
  const ink = "#171725",
    skin = "#edc49b",
    work = pose === "working",
    walk = pose === "walking",
    think = pose === "thinking";
  const f = frame % 8,
    step = walk ? [0, 1, 0, -1][f % 4]! : 0;
  if (role === "shogun") {
    // Tall elder: broad straw hat, white beard, flowing ivory robe, walking staff.
    r(5, 19, 4, 3, ink);
    r(10, 19, 4, 3, ink);
    r(5, 20 + step, 4, 2, "#896854");
    r(10, 20 - step, 4, 2, "#896854");
    p("M6 9h7v3h2v7h2v2H3v-2h2V12h1Z", ink);
    p("M7 10h5v3h2v6h1v1H5v-1h1v-6h1Z", "#e9dbb6");
    r(6, 12, 2, 7, "#ad8d62");
    r(11, 11, 2, 8, "#fff0c8");
    r(8, 15, 4, 2, "#bc8140");
    r(9, 16, 2, 3, "#dbad57");
    r(6, 3, 8, 7, ink);
    r(7, 4, 6, 5, skin);
    r(8, 6, 1, 1, ink);
    r(11, 6, 1, 1, ink);
    p("M7 8h6v2h-1v2h-1v1H9v-1H8v-2H7Z", "#e9e9df");
    r(9, 9, 2, 3, "#ffffec");
    p("M7 0h6v1h3v2h3v2H1V3h3V1h3Z", ink);
    p("M7 1h6v1h3v1h2v1H2V3h3V2h2Z", "#d9a857");
    r(5, 3, 10, 1, "#ffe1a0");
    r(17, 9, 1, 13, "#875d3b");
    r(16, 8, 3, 2, "#cda468");
    if (work) {
      const spread = [1, 2, 3, 3, 2, 1][frame % 6]!;
      r(0, 12, 5 + spread, 5, "#80613f");
      r(1, 12, 4 + spread, 4, "#efdfb9");
      r(2, 13, 2 + spread, 1, "#7c8c75");
      const hand = frame % 6 === 4 ? 2 : 0;
      r(13, (tool === "acceptance-scroll" ? 6 : 12) + hand, 3, 2, skin);
      r(15, (tool === "acceptance-scroll" ? 5 : 11) + hand, 2, 2, "#bb5750");
    } else {
      r(2, 12 + step, 3, 5, "#b69b70");
      r(2, 15 + step, 3, 2, skin);
      if (think) r(12, 8, 2, 2, skin);
    }
  } else if (role === "samurai") {
    // Squat blacksmith: bare arms, spiked hair and headband, leather apron, heavy boots.
    r(3, 17 + step, 5, 3, ink);
    r(10, 17 - step, 5, 3, ink);
    r(3, 19 + step, 6, 2, "#69586a");
    r(10, 19 - step, 6, 2, "#69586a");
    r(3, 8, 12, 10, ink);
    r(4, 9, 10, 6, "#c55944");
    r(6, 10, 7, 9, "#78482f");
    r(7, 11, 5, 7, "#ab7044");
    r(8, 12, 2, 1, "#e1aa69");
    r(6, 17, 7, 1, "#563545");
    r(4, 1, 10, 8, ink);
    r(5, 3, 8, 5, skin);
    r(6, 6, 2, 1, ink);
    r(11, 6, 1, 1, ink);
    r(6, 8, 6, 1, "#b17a56");
    p("M3 2V0h3v1h2V0h3v1h3v2h1v2H3Z", "#323044");
    r(3, 4, 13, 2, "#e05e4d");
    r(2, 5, 3, 2, "#f59d67");
    r(15, 5, 3, 1, "#ffb37e");
    r(14, 10, 3, 6, "#be8d66");
    r(14, 10, 2, 4, skin);
    r(14, 15, 3, 2, "#f0c697");
    if (work && tool === "quench") {
      r(0, 12, 5, 3, skin);
      r(-2, 13, 3, 2, "#b7c9d3");
      r(-3, 14 + (f % 2), 2, 5, "#eaba72");
    } else {
      const lift = work
        ? [0, -2, -5, -4, -1, -2][frame % 6]!
        : walk
          ? step
          : think
            ? -2
            : 0;
      r(0, 10 + lift, 4, 4, skin);
      r(-2, 5 + lift, 2, 10, "#a98253");
      r(-5, 3 + lift, 8, 4, ink);
      r(-4, 3 + lift, 6, 3, "#9ba9bc");
      r(-4, 3 + lift, 6, 1, "#eff0da");
    }
  } else if (role === "sentinel") {
    // Compact automaton: floating square visor, single antenna and tracked feet; no humanoid robe.
    r(3, 16 + step, 6, 3, ink);
    r(11, 16 - step, 6, 3, ink);
    r(4, 17 + step, 4, 1, "#8cabaf");
    r(12, 17 - step, 4, 1, "#8cabaf");
    r(4, 8, 12, 9, ink);
    r(5, 9, 10, 7, "#287e83");
    r(6, 9, 8, 2, "#91e8d7");
    r(7, 12, 6, 3, "#143f52");
    r(8 + (work ? f % 3 : 0), 13, 2, 1, "#6fffd8");
    r(2, 1, 16, 8, ink);
    r(3, 2, 14, 6, "#67bfc2");
    r(4, 3, 12, 4, "#183447");
    r(5, 4, 10, 2, "#63f4d0");
    r(10, -2, 1, 3, "#94b7c0");
    r(9, -3, 3, 2, work && f > 5 ? "#ffc670" : "#80ffe1");
    r(1, 10, 3, 5, "#4c999c");
    r(0, 12, 2, 3, "#a1e1d1");
    const extend = work ? [0, 1, 2, 4, 4, 3, 1, 0][f]! : think ? 2 : 0;
    r(16, 10, 3 + extend, 3, "#437c8a");
    r(18 + extend, 9, 3, 5, ink);
    r(19 + extend, 10, 2, 3, "#9bffe1");
    if (work && f >= 2 && f <= 5)
      r(22 + extend, 11, 3 + (f % 2) * 2, 1, "#80ffe1");
  } else {
    // Slender masked archivist: asymmetric hood, long violet cape, side satchel, quill.
    r(6, 19 + step, 3, 3, ink);
    r(10, 19 - step, 3, 3, ink);
    r(6, 21 + step, 4, 1, "#8887a7");
    r(10, 21 - step, 4, 1, "#8887a7");
    p("M6 7h7v4h1v5h2v4H3v-3h1v-6h2Z", ink);
    p("M6 8h6v4h1v5h1v2H4v-3h1v-5h1Z", "#5e4287");
    r(5, 12, 2, 6, "#9262bc");
    r(11, 12, 1, 6, "#b793e2");
    p("M6 0h5v1h3v3h1v6H4V4h1V1h1Z", ink);
    p("M6 2h6v2h1v5H5V4h1Z", "#9b78cd");
    r(6, 4, 6, 4, "#28213f");
    r(7, 5, 1, 1, "#e4d5ff");
    r(10, 5, 1, 1, "#e4d5ff");
    r(2, 13, 4, 5, "#9c7d62");
    r(2, 13, 4, 1, "#d1b89c");
    r(6, 10, 1, 5, "#c2a580");
    const page = work ? [0, 0, 1, 2, 3, 2, 1, 0][f]! : 0;
    r(10, 13, 8, 5, ink);
    r(11, 13, 6, 4, "#e6d2b4");
    r(13 - page, 12 - page, 1, 5 + page, "#fff0d1");
    r(14, 14, 2, 1, "#8a779c");
    const penY = work && f > 4 ? 1 : 0;
    r(17, 10 + penY, 1, 7, "#bea7df");
    r(17, 9 + penY, 3, 2, "#f3e1f7");
    if (think) r(12, 8, 2, 2, "#dac2bd");
  }
  return out.join("");
}
/** Side and rear sprites are drawn separately: the face, gear and silhouette turn together. */
function directionalPixels(
  role: Role,
  frame: number,
  pose: Pose,
  direction: Direction,
  tool?: ToolId,
) {
  const out: string[] = [],
    ink = "#171725",
    skin = "#edc49b";
  const r = (x: number, y: number, w: number, h: number, c: string) =>
    out.push(
      `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${c}"/>`,
    );
  const p = (d: string, c: string) => out.push(`<path d="${d}" fill="${c}"/>`);
  const back = direction === "north",
    step = pose === "walking" ? [0, 1, 0, -1][frame % 4]! : 0;
  if (role === "shogun") {
    r(6, 20 + step, 4, 2, ink);
    r(11, 20 - step, 4, 2, ink);
    p("M7 9h6v3h2v7h1v2H4v-3h2v-6h1Z", ink);
    p("M8 10h4v3h2v6h1v1H5v-2h2v-5h1Z", "#d4c39c");
    r(8, 12, 2, 7, "#f7e8c4");
    r(6, 16, 9, 2, "#b68344");
    r(6, 3, 8, 7, ink);
    r(7, 4, 6, 6, back ? "#c9ccc2" : skin);
    if (!back) {
      r(12, 6, 1, 1, ink);
      r(13, 7, 2, 2, skin);
      r(11, 9, 3, 3, "#ffffe5");
    } else {
      r(7, 8, 6, 3, "#f2eed8");
      r(9, 10, 2, 2, "#b9bdb5");
    }
    p("M7 0h6v1h3v2h3v2H1V3h3V1h3Z", ink);
    p("M7 1h6v1h3v1h2v1H2V3h3V2h2Z", "#d9a857");
    r(5, 3, 10, 1, "#ffe1a0");
    r(back ? 3 : 16, 9, 1, 13, "#a87846");
    r(back ? 2 : 15, 8, 3, 2, "#e5bf76");
    r(11, 12 + step, 3, 5, "#e9dbb6");
    r(11, 16 + step, 3, 2, skin);
    if (back) {
      r(5, 12, 2, 7, "#ad8d62");
      r(9, 13, 3, 2, "#bc8140");
    }
  } else if (role === "samurai") {
    r(5, 17 + step, 5, 4, ink);
    r(11, 17 - step, 5, 4, ink);
    r(5, 19 + step, 6, 2, "#69586a");
    r(11, 19 - step, 6, 2, "#69586a");
    r(4, 8, 11, 10, ink);
    r(5, 9, 9, 8, "#b74c3c");
    if (back) {
      p("M5 9h2l6 8h-2Zm7 0h2l-7 8H5Z", "#daa174");
      r(6, 16, 8, 2, "#815435");
    } else {
      r(11, 10, 3, 9, "#9c653d");
      r(7, 10, 4, 6, skin);
      r(7, 15, 5, 2, "#b37e5a");
    }
    r(4, 1, 10, 8, ink);
    r(5, 3, 8, 5, back ? "#323044" : skin);
    p("M3 2V0h3v1h2V0h3v1h3v2h1v2H3Z", "#323044");
    r(3, 4, 12, 2, "#e05e4d");
    r(2, 5, 3, 2, "#f59d67");
    if (back) {
      r(8, 6, 2, 4, "#fa865c");
      r(10, 6, 3, 2, "#bc463d");
      r(14, 10, 3, 6, skin);
    } else {
      r(12, 6, 1, 1, ink);
      r(13, 7, 2, 1, skin);
      r(12, 8, 2, 1, "#b17a56");
    }
    r(2, 12 + step, 2, 9, "#b78a52");
    r(0, 11 + step, 6, 3, "#adbbc7");
    r(0, 11 + step, 6, 1, "#e4e9d4");
  } else if (role === "sentinel") {
    r(4, 16 + step, 6, 3, ink);
    r(11, 16 - step, 6, 3, ink);
    r(5, 17 + step, 4, 1, "#8cabaf");
    r(12, 17 - step, 4, 1, "#8cabaf");
    r(5, 8, 11, 9, ink);
    r(6, 9, 9, 7, "#287e83");
    r(7, 9, 7, 2, "#91e8d7");
    r(3, 1, 14, 8, ink);
    r(4, 2, 12, 6, "#67bfc2");
    r(10, -2, 1, 3, "#94b7c0");
    r(9, -3, 3, 2, "#80ffe1");
    if (back) {
      r(6, 4, 8, 1, "#2e6376");
      r(6, 6, 8, 1, "#2e6376");
      r(8, 12, 5, 3, "#184656");
      r(9, 13, 3, 1, "#eecc7e");
      r(2, 10, 3, 5, "#4c999c");
    } else {
      r(12, 3, 4, 4, "#183447");
      r(14, 4, 3, 2, "#63f4d0");
      r(7, 11, 4, 4, "#73bab8");
    }
    const extend =
      pose === "working" ? [0, 1, 2, 4, 4, 3, 1, 0][frame % 8]! : 0;
    r(15, 10, 4 + extend, 3, "#437c8a");
    r(18 + extend, 9, 3, 5, ink);
    r(19 + extend, 10, 2, 3, "#9bffe1");
    if (tool === "scanner" && pose === "working")
      r(22 + extend, 11, 4, 1, "#a2ffe9");
  } else {
    r(7, 19 + step, 3, 3, ink);
    r(11, 19 - step, 3, 3, ink);
    p("M6 7h7v4h1v5h2v4H3v-3h1v-6h2Z", ink);
    p("M6 8h6v4h1v5h1v2H4v-3h1v-5h1Z", "#684691");
    r(5, 12, 2, 6, "#996ac5");
    r(11, 13, 1, 6, "#b79adb");
    p("M6 0h5v1h3v3h1v6H4V4h1V1h1Z", ink);
    p("M6 2h6v2h1v5H5V4h1Z", "#9b78cd");
    if (back) {
      r(6, 4, 2, 4, "#b996df");
      r(9, 5, 3, 4, "#755299");
      p("M5 10h2l6 6v2h-2Z", "#c6a87d");
    } else {
      r(11, 4, 3, 4, "#28213f");
      r(12, 5, 1, 1, "#e4d5ff");
      r(12, 12 + step, 3, 4, "#a98ad0");
      r(14, 13 + step, 3, 5, "#e6d2b4");
    }
    r(3, 14, 5, 5, "#9c7d62");
    r(3, 14, 5, 1, "#dfc5a2");
    r(5, 16, 1, 1, "#d8ae6c");
  }
  return `<g data-sprite-direction="${direction}" transform="${direction === "west" ? "translate(20 0) scale(-1 1)" : ""}">${out.join("")}</g>`;
}
export function footBaseline(role: Role) {
  return role === "sentinel" ? 19 : role === "samurai" ? 21 : 22;
}
export function characterMarkup(
  role: Role,
  x: number,
  y: number,
  frame = 0,
  pose: Pose = "idle",
  tool?: ToolId,
  facing: Direction = "south",
) {
  const scale = 2; // 22 logical pixels = 44 world pixels, down from the former 60-pixel figures.
  return `<g transform="translate(${Math.round(x)} ${Math.round(y)})"><path d="M-10 0h19l12 7H-2Z" fill="#0b1220" opacity=".28"/><ellipse cy="1" rx="12" ry="3" fill="#101422" opacity=".6"/><g transform="translate(${-10 * scale} ${-footBaseline(role) * scale}) scale(${scale})"><g data-facing="${facing}">${characterPixels(role, frame, pose, tool, facing)}</g></g></g>`;
}
export function motionAllowed(
  dark: boolean,
  paused: boolean,
  reduced: boolean,
  visible: boolean,
) {
  return dark && !paused && !reduced && visible;
}
export function moveToward(from: Point, to: Point, distance: number): Point {
  const dx = to.x - from.x,
    dy = to.y - from.y,
    length = Math.hypot(dx, dy);
  if (!length || distance >= length) return { ...to };
  return {
    x: from.x + (dx / length) * distance,
    y: from.y + (dy / length) * distance,
  };
}
