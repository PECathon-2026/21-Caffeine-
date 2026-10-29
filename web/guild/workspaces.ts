import { workspaceOffsets, type Workspace } from "./layout.ts";
import { gradeScenery } from "./materials.ts";
export interface DepthObject {
  id: string;
  y: number;
  markup: string;
}
export function workspaceLayers(workspace: Workspace, dark: boolean) {
  const shift = workspaceOffsets[workspace];
  const legacy =
    workspace === "samurai"
      ? { x: -20, y: 0 }
      : workspace === "shadow"
        ? { x: -380, y: 80 }
        : { x: 0, y: 0 };
  const dx = shift.x + legacy.x,
    dy = shift.y + legacy.y;
  let o: string[] = [];
  const objects: DepthObject[] = [];
  const group = (s: string) =>
    `<g transform="translate(${dx} ${dy})">${gradeScenery(s, dark)}</g>`;
  const r = (x: number, y: number, w: number, h: number, c: string) =>
    o.push(`<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${c}"/>`);
  const p = (d: string, c: string) => o.push(`<path d="${d}" fill="${c}"/>`);
  const ink = dark ? "#191a2b" : "#4b6053",
    floor = dark ? "#635f51" : "#b5a184";
  function object(id: string, y: number, draw: () => void) {
    const previous = o;
    o = [];
    draw();
    objects.push({
      id: `${workspace}-${id}`,
      y: y + dy,
      markup: group(o.join("")),
    });
    o = previous;
  }
  const lantern = (x: number, y: number) =>
    object(`lantern-${x}`, y + 26, () => {
      r(x, y, 4, 26, "#79674d");
      r(x - 4, y - 5, 12, 3, ink);
      r(x - 3, y - 2, 10, 12, "#c9864f");
      r(x - 1, y, 6, 8, "#f3d88c");
      r(x - 4, y + 10, 12, 3, ink);
    });
  const roof = (x: number, y: number, w: number) => {
    p(`M${x - 14} ${y}h12v-6h14v-7h${w - 24}v7h14v6h12v9h-${w + 28}Z`, ink);
    r(x - 5, y - 11, w + 10, 12, "url(#guild-roof)");
    r(x - 6, y - 3, w + 12, 4, dark ? "#414860" : "#6f8976");
    for (let a = x; a < x + w; a += 12)
      r(a, y + 2, 8, 2, dark ? "#8b7e97" : "#adba99");
  };
  const crate = (x: number, y: number) => {
    r(x, y, 20, 17, "#3c2c34");
    r(x + 2, y + 2, 16, 13, "#90614e");
    r(x + 3, y + 3, 14, 2, "#c08d62");
    r(x + 4, y + 7, 12, 1, "#5d3d39");
    r(x + 4, y + 12, 12, 1, "#5d3d39");
    r(x + 9, y + 3, 2, 11, "#bd895f");
  };
  if (workspace === "shogun") {
    // The Shogun retains the council pagoda, with an outdoor strategy table.
    object("pagoda", 198, () => {
      for (let i = 0; i < 3; i++) {
        const w = 114 - i * 28,
          x = 450 - w / 2,
          y = 185 - i * 38;
        r(x, y - 34, w, 34, "#ac9278");
        r(x + 5, y - 28, w - 10, 23, "#d4bb95");
        for (let k = x + 11; k < x + w - 6; k += 22) {
          r(k, y - 26, 12, 18, "#746548");
          r(k + 2, y - 24, 8, 14, "#e9c786");
          r(k + 6, y - 24, 1, 14, "#876e4f");
        }
        roof(x, y - 36, w);
        r(x, y - 3, w, 4, "#795746");
      }
      r(378, 189, 144, 9, "#8e9780");
    });
    object("council-table", 232, () => {
      r(398, 215, 104, 23, "#664d3a");
      r(395, 209, 110, 12, "#b39462");
      r(404, 211, 91, 7, "#e2d0a1");
      r(414, 212, 18, 2, "#8e9d78");
      r(447, 212, 28, 3, "#839782");
      r(434, 214, 7, 3, "#c27554");
      r(403, 231, 7, 13, "#503f33");
      r(490, 231, 7, 13, "#503f33");
      r(495, 209, 17, 8, "#eed8a8");
      r(493, 208, 3, 10, "#936949");
      r(512, 208, 3, 10, "#936949");
      r(499, 212, 9, 1, "#aa7757");
    });
    lantern(374, 216);
    lantern(522, 216);
  } else if (workspace === "samurai") {
    // SAMURAI FORGE: a cutaway smithy with a brick furnace, chimney, anvil and quench barrel.
    r(94, 395, 240, 12, ink);
    r(97, 291, 234, 104, floor);
    for (let y = 296; y < 390; y += 12) {
      r(99, y, 230, 1, dark ? "#746f59" : "#c2ae8c");
      for (let x = 103 + (y % 24); x < 326; x += 30)
        r(x, y, 1, 11, dark ? "#504d43" : "#a49176");
    }
    object("forge-shell", 312, () => {
      r(105, 255, 218, 57, dark ? "#594d43" : "#9d856a");

      roof(95, 247, 239);
      r(134, 219, 27, 43, "#4c5251");
      for (let y = 222; y < 259; y += 8) {
        r(136, y, 23, 1, "#8b8270");
        r(y % 16 ? 143 : 152, y, 1, 7, "#7c7c69");
      }
      r(130, 218, 35, 6, "#7c7e69");
    });
    object("furnace", 360, () => {
      r(121, 282, 65, 69, "#343638");
      for (let y = 286; y < 348; y += 9)
        for (let x = 125; x < 182; x += 14) {
          r(x, y, 12, 7, "#746154");
          r(x, y, 12, 1, "#9b7d5f");
        }
      r(132, 303, 44, 43, "#2b2427");
      r(138, 309, 32, 34, "#a84331");
      o.push('<g data-effect="fire">');
      p("M139 342v-13h5v-14h5v9h4v-19h5v18h5v-9h4v28Z", "#f39143");
      p("M146 342v-10h4v-10h4v11h5v-6h4v15Z", "#ffe7a1");
      o.push("</g>");
      r(127, 348, 55, 5, "#242e32");
      r(116, 354, 77, 6, "#928374");
    });
    object("anvil", 352, () => {
      // Anvil: horn, polished face, narrow waist and heavy foot.
      r(211, 343, 45, 9, "#777b76");
      p("M205 322h53v6h-8v7h-13v8h-13v-8h-12v-5h-7Z", "#37434b");
      r(208, 322, 48, 3, "#c1c8bd");
      r(214, 325, 33, 3, "#738c90");
    });
    object("tool-bench", 336, () => {
      r(275, 307, 36, 9, "#ad895a");
      r(278, 315, 5, 21, "#67523d");
      r(302, 315, 5, 21, "#67523d");
      r(282, 301, 24, 5, "#b9c3b9");
      r(287, 295, 7, 6, "#526b71");
      r(300, 284, 3, 18, "#c29d67");
      r(296, 282, 11, 5, "#87938c");
    });
    object("barrel", 381, () => {
      o.push('<g transform="translate(-10 0)">');
      r(282, 352, 25, 27, "#795d42");
      r(281, 352, 27, 5, "#93aaa2");
      r(285, 355, 19, 4, "#507b83");
      r(281, 369, 27, 3, "#363f43");
      r(286, 357, 2, 24, "#aa8559");
      o.push("</g>");
    });
    for (let i = 0; i < 4; i++) {
      r(106 + i * 9, 374, 8, 12, "#70513b");
      r(107 + i * 9, 374, 6, 3, "#b1915d");
    }
    o.push(
      '<g data-effect="smoke" fill="#98a79e" opacity=".45"><rect x="141" y="202" width="10" height="9"/><rect x="149" y="186" width="13" height="8"/><rect x="143" y="168" width="16" height="8"/></g>',
    );
    object("forge-posts", 394, () => {
      r(105, 255, 7, 139, "#917149");
      r(315, 255, 7, 139, "#917149");
    });
    object("supply-crates", 391, () => {
      crate(104, 356);
      crate(182, 374);
    });
    for (let i = 0; i < 5; i++) {
      r(199 + i * 9, 277, 3, 23, "#bf966a");
      r(197 + i * 9, 275, 7, 5, i % 2 ? "#9fadae" : "#c7c5b0");
    }
    object("workpiece", 353, () => {
      r(231, 321, 17, 2, "#ffb962");
      r(233, 321, 12, 1, "#fff0b9");
    });
  } else if (workspace === "sentinel") {
    // SENTINEL DOJO: open tatami hall, shoji partitions and three physical check stations.
    r(594, 394, 218, 10, ink);
    r(598, 293, 210, 101, dark ? "#99916a" : "#c4ba88");
    for (let y = 298; y < 391; y += 23)
      for (let x = 602; x < 805; x += 49) {
        r(x, y, 46, 20, dark ? "#8a8e6b" : "#b2b886");
        r(x + 2, y + 2, 42, 1, dark ? "#b0b08a" : "#d4d19d");
      }
    object("dojo-shell", 299, () => {
      r(600, 254, 204, 45, "#8f7459");
      for (let x = 605; x < 804; x += 30) {
        r(x, 259, 25, 33, dark ? "#c5bf9a" : "#e0d8b8");
        r(x + 12, 259, 2, 33, "#8b8064");
        r(x, 273, 25, 2, "#8b8064");
      }
      roof(591, 247, 223);
    });
    object("scanner", 360, () => {
      // A scan gantry with a sample document and an illuminated inspection bed.
      r(748, 307, 42, 6, "#334856");
      r(750, 313, 4, 42, "#527c84");
      r(784, 313, 4, 42, "#527c84");
      r(752, 308, 34, 2, "#a6dbce");
      r(755, 320, 27, 31, "#254649");
      r(762, 327, 14, 19, "#d8deca");
      r(765, 331, 8, 1, "#677c76");
      r(765, 335, 6, 1, "#677c76");
      r(747, 354, 45, 6, "#718e8c");
      r(751, 354, 36, 2, "#a7bca5");
    });
    object("test-monitors", 340, () => {
      for (let i = 0; i < 3; i++) {
        r(623 + i * 34, 306, 25, 20, ink);
        r(627 + i * 34, 309, 17, 12, i === 2 ? "#c47d59" : "#759e80");
        r(634 + i * 34, 329, 4, 9, "#4e5850");
        r(629 + i * 34, 337, 14, 3, "#b4a879");
      }
      o.push(
        '<g data-effect="checks" fill="#e4efd1"><path d="m630 316 3 3 6-6h-2l-4 4-1-1Z"/><path d="m664 316 3 3 6-6h-2l-4 4-1-1Z"/></g>',
      );
    });
    lantern(589, 345);
    lantern(814, 345);
    object("dojo-posts", 391, () => {
      r(600, 253, 6, 138, "#796449");
      r(798, 253, 6, 138, "#796449");
    });
    object("dojo-crate", 390, () => crate(773, 373));
  } else if (workspace === "shadow") {
    // SHADOW ARCHIVE: exposed bookcases, a scroll-covered desk and a magnifying lens.
    r(490, 500, 186, 9, ink);
    r(491, 423, 184, 77, dark ? "#5b6265" : "#a5a595");
    object("bookshelves", 432, () => {
      r(493, 404, 179, 44, dark ? "#3d4350" : "#777c83");
      for (let row = 0; row < 2; row++) {
        r(502, 413 + row * 17, 158, 3, "#a18b68");
        for (let col = 0; col < 16; col++) {
          const colors = ["#7a8e7a", "#a58480", "#9a91b0", "#b69e75"];
          r(
            504 + col * 10,
            400 + row * 17,
            6 + (col % 3),
            13,
            colors[col % 4]!,
          );
          r(505 + col * 10, 402 + row * 17, 1, 9, "#d4c7a2");
        }
      }
      r(492, 399, 5, 101, "#6f6659");
      r(668, 399, 5, 101, "#6f6659");
      r(489, 395, 189, 7, "#859185");
    });
    object("review-table", 476, () => {
      r(543, 459, 96, 9, "#a08764");
      r(548, 468, 5, 23, "#4f4741");
      r(629, 468, 5, 23, "#4f4741");
      r(551, 452, 29, 7, "#e0d1ad");
      r(556, 453, 17, 1, "#8c8e79");
      r(583, 451, 25, 8, "#cabc9d");
      r(588, 453, 14, 1, "#788879");
      r(616, 451, 4, 9, "#363c48");
      r(612, 445, 11, 8, "#aec8b7");
      r(614, 447, 7, 4, "#4e7380");
    });
    lantern(648, 444);
  } else if (workspace === "campaign") {
    // E: THE LIVING CAMPAIGN. Shared mission wall, relay desk, autonomy console, approval seal.
    r(510, 598, 315, 12, "#171b2d");
    r(514, 482, 307, 115, "url(#guild-wood)");
    r(514, 472, 307, 10, "#585472");
    r(520, 484, 4, 109, "#a98b6b");
    r(811, 484, 4, 109, "#a98b6b");
    object("mission-board", 546, () => {
      // Shared board: task cards move through a visible branching plan.
      r(535, 484, 115, 62, "#191b30");
      r(538, 487, 109, 56, "#293247");
      for (let i = 0; i < 3; i++) {
        r(544 + i * 33, 492, 27, 7, ["#b69057", "#618e89", "#8e739f"][i]!);
        r(544 + i * 33, 502, 27, 18, "#ded1b5");
        r(548 + i * 33, 507, 17, 2, "#7e7c83");
        r(548 + i * 33, 511, 12, 1, "#7e7c83");
      }
      p("M556 525h2v7h66v-7h2v9h-70Z", "#cbbb85");
      r(587, 528, 9, 3, "#db9974");
    });
    object("relay-table", 576, () => {
      // Role sockets: gold planning, ember building, teal checking, violet reviewing.
      r(548, 551, 123, 11, "#aa8b64");
      r(553, 562, 5, 24, "#4c3e42");
      r(661, 562, 5, 24, "#4c3e42");
      for (let i = 0; i < 4; i++) {
        r(557 + i * 28, 541, 17, 10, "#1c2335");
        r(
          560 + i * 28,
          543,
          11,
          5,
          ["#e6bd69", "#e78561", "#56d9bc", "#b791ec"][i]!,
        );
      }
    });
    object("console", 569, () => {
      // Console, stop switch, iteration counter and human seal.
      r(689, 516, 106, 12, "#7f7b87");
      r(695, 527, 6, 46, "#443b50");
      r(783, 527, 6, 46, "#443b50");
      r(704, 485, 60, 33, "#202238");
      r(708, 489, 52, 24, "#3b5269");
      r(713, 494, 12, 3, "#7fc1c4");
      r(713, 501, 32, 2, "#b5dacf");
      r(713, 506, 22, 2, "#749fba");
      r(714, 521, 44, 4, "#bbb6ad");
      r(769, 511, 15, 9, "#332536");
      r(772, 511, 9, 5, "#e76b68");
      r(718, 545, 63, 24, "#282436");
      r(723, 550, 12, 13, "#daae66");
      r(739, 550, 12, 13, "#daae66");
      r(755, 550, 17, 13, "#5c536c");
    });
    object("seal-bench", 585, () => {
      r(688, 578, 111, 7, "#8e7488");
      r(693, 580, 101, 2, "#c7a7a0");
    });
    lantern(524, 561);
    lantern(804, 560);
  }
  return { base: group(o.join("")), objects };
}
