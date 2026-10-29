import { courtyard } from "./gardens.ts";
import {
  WORLD,
  inWorkspace,
  roleOrder,
  stations,
  workspaceBounds,
  type Workspace,
} from "./layout.ts";
import { materials, gradeScenery } from "./materials.ts";
import { characterMarkup } from "./characters.ts";
import { walkEdges, walkNodes } from "./routes.ts";
import { workspaceLayers, type DepthObject } from "./workspaces.ts";

/** Ground, solid objects and lighting are separate so moving feet determine depth. */
export function guildLayers(dark = true) {
  const o: string[] = [],
    objects: DepthObject[] = [],
    lights: string[] = [];
  const r = (x: number, y: number, w: number, h: number, c: string) =>
    o.push(`<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${c}"/>`);
  const p = (d: string, c: string) => o.push(`<path d="${d}" fill="${c}"/>`);
  const ink = dark ? "#142432" : "#536b5b";
  r(-500, -400, 2160, 1400, dark ? "#10101d" : "#c8e1e5");
  p(
    "M0 118V92h86V62h70v27h101V70h75V40h78v39h71V58h84v33h117V70h94V43h76v27h78v29h112V68h85v50Z",
    dark ? "#1c2537" : "#98b9b3",
  );
  r(-500, 110, 2160, 890, "url(#guild-grass)");
  if (dark)
    for (let i = 0; i < 40; i++)
      r(
        (i * 137 + 33) % WORLD.width,
        12 + ((i * 37) % 78),
        2,
        2,
        i % 3 ? "#688894" : "#bdc8c3",
      );
  p(
    "M1090 18h22v6h8v10h4v18h-4v9h-8v5h-22v-5h-8v-9h-4V34h4V24h8Z",
    dark ? "#f2dfad" : "#fff1c3",
  );
  // Boundary stream and small planted edges frame a wide, usable campus.
  r(1126, 128, 18, 486, dark ? "#204d64" : "#699b9a");
  for (let y = 144; y < 620; y += 24)
    r(1130, y, 9, 2, dark ? "#44849a" : "#c0d9bf");
  r(1122, 128, 3, 488, dark ? "#4a686a" : "#99af94");
  // The visible path is generated from the exact same edges used by routing.
  for (const e of walkEdges.filter((e) => e.outdoor)) {
    const a = walkNodes[e.from]!,
      b = walkNodes[e.to]!;
    o.push(
      `<path data-walkway="${e.from}:${e.to}" d="M${a.x} ${a.y}L${b.x} ${b.y}" fill="none" stroke="${ink}" stroke-width="34" stroke-linecap="square"/>`,
    );
  }
  for (const e of walkEdges.filter((e) => e.outdoor)) {
    const a = walkNodes[e.from]!,
      b = walkNodes[e.to]!;
    o.push(
      `<path d="M${a.x} ${a.y}L${b.x} ${b.y}" fill="none" stroke="url(#guild-stone)" stroke-width="28" stroke-linecap="square"/>`,
    );
  }
  o.push(`<g transform="translate(130 -10)">${courtyard(dark)}</g>`);
  // Corner planting is deliberately outside the pedestrian network.
  const tree = (x: number, y: number, cherry = false) => {
    const c = cherry
      ? dark
        ? "#bc648d"
        : "#d99a9e"
      : dark
        ? "#2e715a"
        : "#618c63";
    o.push(
      `<ellipse cx="${x + 7}" cy="${y + 2}" rx="23" ry="6" fill="#071b24" opacity=".25"/>`,
    );
    const shape = cherry
      ? `M${x - 24} ${y - 43}h9v-9h22v6h16v10h7v19h-12v7h-19v-5h-20v-9h-9v-13h6Z`
      : `M${x - 3} ${y - 61}h6v9h6v8h8v9h9v11h-6v7h-39v-7h-8v-10h9v-10h9v-8h6Z`;
    objects.push({
      id: `tree-${x}-${y}`,
      y,
      markup: `<g><rect x="${x - 3}" y="${y - 32}" width="6" height="32" fill="#87694f"/><path d="${shape}" fill="${c}"/><path d="M${x - 12} ${y - 42}h15v4h-15Zm16 14h13v3h-13Z" fill="${cherry ? "#f0a2b2" : "#79ab7e"}"/></g>`,
    });
  };
  for (const [x, y] of [
    [42, 182],
    [79, 143],
    [381, 173],
    [444, 111],
    [710, 122],
    [768, 193],
    [1151, 259],
    [58, 440],
    [91, 550],
    [1094, 494],
    [1080, 579],
    [394, 623],
    [771, 624],
  ])
    tree(x!, y!);
  for (const [x, y] of [
    [99, 304],
    [757, 272],
    [418, 466],
    [728, 508],
  ])
    tree(x!, y!, true);
  const rooms = Object.keys(workspaceBounds) as Workspace[];
  for (const workspace of rooms) {
    const room = workspaceLayers(workspace, dark),
      b = workspaceBounds[workspace];
    o.push(`<g data-workspace="${workspace}">${room.base}</g>`);
    objects.push(...room.objects);
    o.push(
      `<path d="M${b.x + 16} ${b.y + b.height - 4}h${b.width - 20}l9 6H${b.x + 25}Z" fill="#081521" opacity=".24"/>`,
    );
  }
  // All rooms are entered through their open fronts, with visible stone steps.
  for (const [workspace, x, bottom, doorY] of [
    ["shogun", 580, 227, 266],
    ["samurai", 316, 509, 548],
    ["sentinel", 895, 499, 540],
    ["shadow", 212, 218, 258],
    ["campaign", 970, 217, 242],
  ] as const) {
    if (workspace === "shogun") {
      o.push(
        `<rect x="497" y="172" width="168" height="55" fill="url(#guild-stone)"/><path d="M497 172h168v2H497Z" fill="#abb29a"/>`,
      );
    }
    r(x - 18, bottom, 36, doorY - bottom, ink);
    for (let y = bottom; y < doorY; y += 6) {
      r(x - 18, y, 36, 4, dark ? "#6e7681" : "#bfc3a8");
      r(x - 18, y, 36, 1, dark ? "#a2a89b" : "#e0dbc0");
    }
  }
  // Sign labels occupy dedicated verges, not pedestrian routes.
  for (const [workspace, label, x, y] of [
    ["shogun", "SHOGUN’S COUNCIL", 580, 247],
    ["samurai", "SAMURAI FORGE", 264, 580],
    ["sentinel", "SENTINEL DOJO", 944, 580],
    ["shadow", "SHADOW ARCHIVE", 224, 264],
    ["campaign", "LIVING CAMPAIGN", 956, 256],
  ] as const) {
    const color = {
      shogun: "#debc7c",
      samurai: "#e6a180",
      sentinel: "#8bdbbf",
      shadow: "#bf9fdf",
      campaign: "#bcb8e1",
    }[workspace];
    o.push(
      `<text class="world-label" x="${x}" y="${y}" text-anchor="middle" fill="${dark ? color : "#344c45"}" font-family="Pixel,monospace" font-size="9" letter-spacing="1">${label}</text>`,
    );
  }
  o.push(
    `<text class="world-label" x="580" y="463" text-anchor="middle" fill="${dark ? "#bbc5b7" : "#294534"}" font-family="Pixel,monospace" font-size="8" letter-spacing="1.2">SHARED COURTYARD</text>`,
  );
  // Four sockets give actual handoffs a visible destination in the centre of the grounds.
  for (const [x, color] of [
    [552, "#debc7c"],
    [608, "#8bdbbf"],
  ] as const) {
    o.push(
      `<path d="M${x - 13} 417h26v7h-26Z" fill="none" stroke="${color}" stroke-width="1" opacity=".6"/>`,
    );
  }
  for (const x of [530, 630]) {
    objects.push({
      id: `court-lantern-${x}`,
      y: 416,
      markup: `<g><rect x="${x}" y="395" width="4" height="21" fill="#907253"/><rect x="${x - 4}" y="384" width="12" height="15" fill="#584346"/><rect x="${x - 2}" y="387" width="8" height="9" fill="#f2d791"/></g>`,
    });
  }
  r(50, 626, 1060, 3, dark ? "#3c585a" : "#91a288");
  for (let x = 60; x < 1110; x += 24)
    r(x, 632, 15, 1, dark ? "#547267" : "#a9b79b");
  // Light stays with the room when its position changes.
  if (dark) {
    for (const [workspace, x, y, color, radius] of [
      ["shogun", 450, 211, "#ffcb7e", 67],
      ["samurai", 133, 324, "#ff913f", 61],
      ["sentinel", 677, 330, "#4ddec1", 60],
      ["shadow", 210, 541, "#b78be8", 54],
      ["campaign", 742, 540, "#81b3e4", 55],
    ] as const) {
      const point = inWorkspace(workspace, { x, y });
      for (let ring = 5; ring > 0; ring--)
        lights.push(
          `<ellipse cx="${point.x}" cy="${point.y}" rx="${(radius * ring) / 5}" ry="${(radius * ring) / 7}" fill="${color}" opacity="${workspace === "samurai" ? 0.055 : 0.03}"/>`,
        );
    }
  }
  const ground = gradeScenery(o.join(""), dark);
  return {
    ground,
    objects: dark
      ? objects
      : objects.map((object) => ({
          ...object,
          markup: gradeScenery(object.markup, false),
        })),
    lights: `<g class="scenery-light" style="pointer-events:none;mix-blend-mode:screen">${lights.join("")}</g>`,
  };
}
export function guildWorld(dark = true, withCharacters = true) {
  const layers = guildLayers(dark),
    items = [...layers.objects];
  if (withCharacters)
    for (const role of roleOrder)
      items.push({
        id: role,
        y: stations[role].y,
        markup: characterMarkup(
          role,
          stations[role].x,
          stations[role].y,
          0,
          "working",
        ),
      });
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${WORLD.width} ${WORLD.height}" shape-rendering="crispEdges" role="img" aria-label="Five workshops around connected paths and a shared handoff courtyard">${materials(dark)}${layers.ground}${items
    .sort((a, b) => a.y - b.y)
    .map((i) => i.markup)
    .join("")}${layers.lights}</svg>`;
}
