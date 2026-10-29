/** Hand-placed landscape elements. Every path is kept clear for the guild's routes. */
export function courtyard(dark: boolean) {
  const o: string[] = [];
  const r = (
    x: number,
    y: number,
    w: number,
    h: number,
    c: string,
    opacity = 1,
  ) =>
    o.push(
      `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${c}" opacity="${opacity}"/>`,
    );
  const p = (d: string, c: string, opacity = 1) =>
    o.push(`<path d="${d}" fill="${c}" opacity="${opacity}"/>`);
  const edge = dark ? "#101f2c" : "#657865",
    stone = dark ? "#485365" : "#b9bba6",
    highlight = dark ? "#6c7c88" : "#d9dbc2";
  // Raised courtyard: an octagonal compass connects all four circulation branches.
  p("M414 392h72l24 24v28l-24 24h-72l-24-24v-28Z", edge);
  p("M414 389h72l24 24v28l-24 24h-72l-24-24v-28Z", stone);
  p(
    "M417 395h66l21 21v22l-21 21h-66l-21-21v-22Z",
    dark ? "#383e50" : "#a2ad9e",
  );
  p("M431 407h38l16 16v8l-16 16h-38l-16-16v-8Z", dark ? "#75827f" : "#d1c9ae");
  p("M433 410h34l14 14v5l-14 14h-34l-14-14v-5Z", dark ? "#394754" : "#919f93");
  p(
    "M448 412h4v8h4v4h8v4h-8v4h-4v8h-4v-8h-4v-4h-8v-4h8v-4h4Z",
    dark ? "#ccb780" : "#ece1b9",
  );
  r(446, 422, 8, 8, dark ? "#273947" : "#91a191");
  r(448, 424, 4, 4, dark ? "#e3c78e" : "#e8d8aa");
  for (const [x, y] of [
    [404, 419],
    [490, 419],
    [443, 398],
    [443, 450],
  ]) {
    r(x!, y!, 12, 2, highlight);
    r(x! + 3, y! + 4, 6, 1, highlight, 0.6);
  }
  o.push('<g transform="translate(-35 20)">');
  // Pond, stone edging and reeds: left of the north path, outside the forge.
  p(
    "M356 298h36v8h16v12h8v35h-8v12h-16v8h-36v-8h-15v-12h-7v-35h7v-12h15Z",
    edge,
  );
  p(
    "M356 297h36v8h16v12h7v32h-8v12h-16v8h-33v-8h-15v-12h-7v-31h7v-12h13Z",
    stone,
  );
  p(
    "M359 304h30v8h14v9h6v25h-8v10h-14v7h-26v-7h-13v-12h-5v-22h7v-10h9Z",
    dark ? "#173d54" : "#5c9e9c",
  );
  p(
    "M359 311h29v6h14v15h-9v-10h-29v5h-17v-8h12Z",
    dark ? "#26607a" : "#83bab0",
  );
  r(355, 331, 19, 2, dark ? "#5697a6" : "#b7d8c2");
  r(383, 346, 17, 2, dark ? "#5697a6" : "#b7d8c2");
  o.push('<g data-effect="koi">');
  p("M369 338h7v3h4v3h-3v2h-7v-2h-4v-3h3Z", "#f6c28d");
  r(372, 339, 3, 4, "#dd7863");
  r(368, 340, 1, 1, edge);
  o.push("</g>");
  r(390, 321, 9, 3, dark ? "#4d9476" : "#6da26d");
  r(392, 318, 5, 3, dark ? "#7ab08b" : "#a4c58b");
  for (const [x, y] of [
    [348, 306],
    [396, 306],
    [338, 334],
    [397, 357],
    [357, 366],
  ]) {
    r(x!, y!, 9, 3, highlight);
    r(x! + 1, y! + 3, 7, 3, stone);
  }
  for (let i = 0; i < 4; i++) {
    r(339 + i * 4, 357 - (i % 2) * 3, 2, 11, dark ? "#638c65" : "#718e4f");
    r(338 + i * 4, 356 - (i % 2) * 3, 3, 3, "#b7a67a");
  }
  o.push('</g><g transform="translate(35 35)">');
  // Raked garden across the path: deliberate negative space and a sculpted bonsai.
  r(493, 300, 64, 80, edge);
  r(495, 300, 60, 76, dark ? "#656e69" : "#d4d0ab");
  for (let y = 306; y < 374; y += 6)
    r(499, y, 52, 1, dark ? "#8e9680" : "#b3b994", 0.55);
  p("M514 328h22v5h7v14h-7v5h-22v-5h-7v-14h7Z", dark ? "#9fa38d" : "#e4dcb6");
  p("M517 330h17v5h6v9h-6v5h-17v-5h-7v-9h7Z", dark ? "#616d68" : "#bbc3a4");
  p("M516 339v-8h7v-7h7v6h5v12h-19Z", dark ? "#343e49" : "#758779");
  r(519, 329, 7, 3, highlight);
  r(530, 344, 9, 5, dark ? "#506069" : "#97a490");
  r(532, 343, 5, 2, highlight);
  r(515, 305, 21, 6, "#624542");
  r(518, 310, 15, 6, "#996b58");
  p("M524 305v-19h-5v-5h4v-7h5v15h5v5h-5v11Z", "#967459");
  p(
    "M508 274h11v-6h13v5h11v5h5v7h-17v-3h-10v4h-19v-8h6Z",
    dark ? "#26775c" : "#4c875d",
  );
  p("M510 273h10v-4h11v6h10v4h-16v-2h-15Z", dark ? "#75b480" : "#a0bd78");
  o.push("</g>");
  // Mossy borders, flowers and benches are grouped, rather than scattered noise.
  for (const [x, y, w] of [
    [335, 458, 69],
    [479, 462, 22],
    [318, 591, 96],
    [24, 575, 57],
    [852, 573, 34],
  ]) {
    r(x!, y!, w!, 10, edge);
    r(x! + 2, y!, w! - 4, 7, dark ? "#285f4e" : "#7f9e68");
    for (let a = x! + 6; a < x! + w! - 4; a += 11) {
      r(a, y! + 1, 2, 3, "#cbb784");
      r(a - 1, y!, 4, 2, "#e7ad96");
    }
  }
  for (const [x, y] of [
    [344, 605],
    [570, 530],
  ]) {
    r(x! + 3, y! + 5, 4, 12, edge);
    r(x! + 31, y! + 5, 4, 12, edge);
    r(x!, y!, 38, 7, "#745755");
    r(x!, y!, 38, 2, "#c59677");
    r(x!, y! + 4, 38, 1, "#a07862");
  }
  return `<g class="courtyard-landscape">${o.join("")}</g>`;
}

export function foreground(dark: boolean) {
  const o: string[] = [];
  // A low stone retaining wall frames the entrance; the gate stays open.
  for (const [x, w] of [
    [0, 392],
    [506, 394],
  ]) {
    o.push(
      `<rect x="${x}" y="638" width="${w}" height="12" fill="${dark ? "#243244" : "#8b9b88"}"/><rect x="${x}" y="636" width="${w}" height="3" fill="${dark ? "#70817e" : "#cad0b3"}"/>`,
    );
    for (let a = x! + 4; a < x! + w! - 8; a += 22)
      o.push(
        `<path d="M${a} 642h15v1h-15z" fill="${dark ? "#50605f" : "#acb39a"}"/>`,
      );
  }
  if (dark) {
    o.push('<g data-effect="fireflies" fill="#e4e7aa">');
    for (const [x, y] of [
      [347, 385],
      [536, 397],
      [326, 520],
      [481, 574],
      [315, 202],
      [568, 196],
    ])
      o.push(
        `<rect x="${x}" y="${y}" width="2" height="2"/><rect x="${x! - 2}" y="${y! - 2}" width="6" height="6" opacity=".08"/>`,
      );
    o.push("</g>");
  }
  return o.join("");
}
