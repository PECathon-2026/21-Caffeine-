/** Small repeating pixel materials and banded light, inspired by the upstream renderer. */
export function materials(dark: boolean) {
  const grass = dark ? "#183c38" : "#74a66d";
  const stone = dark ? "#746e7d" : "#c9bb9f";
  return `<defs>
    <pattern id="guild-grass" width="24" height="24" patternUnits="userSpaceOnUse"><rect width="24" height="24" fill="${grass}"/><path d="M3 4h4v1H3zm12 10h5v1h-5zM7 21h3v1H7z" fill="${dark ? "#275247" : "#86b47a"}"/><path d="M0 15h3v1H0zM15 3h3v1h-3zM19 22h4v1h-4z" fill="${dark ? "#12302f" : "#689a61"}"/></pattern>
    <pattern id="guild-stone" width="24" height="16" patternUnits="userSpaceOnUse"><rect width="24" height="16" fill="${stone}"/><path d="M0 0h24v1H0zm0 8h24v1H0zM12 0h1v8h-1zM0 8h1v8H0z" fill="${dark ? "#423f55" : "#8d897d"}"/><path d="M2 2h8v1H2zm12 9h7v1h-7z" fill="${dark ? "#95898e" : "#d8cfb5"}"/></pattern>
    <pattern id="guild-wood" width="32" height="16" patternUnits="userSpaceOnUse"><rect width="32" height="16" fill="${dark ? "#6a4947" : "#a88968"}"/><path d="M0 0h32v1H0zm0 8h32v1H0zM15 0h1v8h-1zM3 8h1v8H3z" fill="#382c38" opacity=".65"/><path d="M2 2h10v1H2zm17 1h11v1H17zM6 10h14v1H6zm18 2h6v1h-6zM5 5h5v1H5z" fill="#c0916e" opacity=".4"/></pattern>
    <pattern id="guild-roof" width="12" height="8" patternUnits="userSpaceOnUse"><rect width="12" height="8" fill="#303348"/><path d="M0 0h12v1H0zm2 2h7v1H2z" fill="#63637e"/><path d="M10 1h2v7h-2z" fill="#1a1b2d"/></pattern>
    <pattern id="guild-mat" width="16" height="8" patternUnits="userSpaceOnUse"><rect width="16" height="8" fill="${dark ? "#586f64" : "#b2b886"}"/><path d="M0 1h16v1H0zm0 3h16v1H0zm0 3h16v1H0z" fill="${dark ? "#809c79" : "#d4d19d"}" opacity=".4"/></pattern>
  </defs>`;
}

export function gradeScenery(markup: string, dark: boolean) {
  const textures: Record<string, string> = {
    [dark ? "#324c46" : "#829477"]: "url(#guild-grass)",
    [dark ? "#3b5348" : "#97a080"]: "url(#guild-grass)",
    [dark ? "#455c4a" : "#a2aa84"]: "url(#guild-grass)",
    [dark ? "#635f51" : "#b5a184"]: "url(#guild-wood)",
    [dark ? "#8a8a6d" : "#d0c29d"]: "url(#guild-stone)",
    [dark ? "#8a8e6b" : "#b2b886"]: "url(#guild-mat)",
  };
  const colors: Record<string, string> = dark
    ? {
        "#17232f": "#10101d",
        "#243b43": "#171d30",
        "#2b464b": "#23283d",
        "#2d4945": "#202e3a",
        "#1d2b34": "#191a2b",
        "#233e3c": "#112b30",
        "#456c58": "#2e6955",
        "#6c8870": "#63a17b",
        "#41666b": "#263d66",
        "#7fa29e": "#558caf",
        "#a57c83": "#ae587e",
        "#d1a09f": "#f292a6",
        "#ac9278": "#a45e4c",
        "#d4bb95": "#c68c63",
        "#e9c786": "#ffd38a",
        "#657f77": "#414860",
        "#8d9b82": "#8b7e97",
        "#99916a": "#384947",
        "#c5bf9a": "#89b59f",
        "#759e80": "#31cbae",
        "#c47d59": "#f0ac58",
        "#5b6265": "#473a5e",
        "#3d4350": "#292637",
        "#9a91b0": "#bca0e4",
        "#75836a": "#457962",
        "#a8a382": "#a39c9d",
        "#594d43": "#493346",
      }
    : {
        "#d99a9e": "#e58fa8",
        "#f0a2b2": "#ffc0cf",
        "#618c63": "#347650",
        "#79ab7e": "#65aa70",
        "#699b9a": "#429b9e",
        "#5b6265": "#424d59",
        "#ac9278": "#aa7957",
        "#d4bb95": "#e8bc84",
      };
  return markup.replace(
    /fill="(#[0-9a-f]{6})"/g,
    (original, c: string) => `fill="${textures[c] ?? colors[c] ?? c}"`,
  );
}

/** The light pools use discrete opacity bands, so the glow has painted pixel depth. */
export function sceneryLights(dark: boolean) {
  if (!dark) return "";
  const items: string[] = [];
  const glow = (
    x: number,
    y: number,
    r: number,
    color: string,
    flat = 1,
    strength = 0.09,
  ) => {
    for (let ring = 9; ring > 0; ring--) {
      const radius = (r * ring) / 9;
      items.push(
        `<ellipse cx="${x}" cy="${y}" rx="${Math.round(radius)}" ry="${Math.round(radius * flat)}" fill="${color}" opacity="${strength}"/>`,
      );
    }
  };
  // Hard-edged pools follow the floor planes, with bright reflected pixels on nearby tools.
  items.push(`<path d="M112 333h46l82 57H88Z" fill="#ffae64" opacity=".07"/>
    <path d="M121 340h33l42 40H100Z" fill="#ffb26a" opacity=".07"/>
    <path d="M626 320h91l35 69H609Z" fill="#67e8c3" opacity=".05"/>
    <path d="M754 351h31l14 36h-63Z" fill="#67f7d2" opacity=".08"/>
    <path d="M166 539h76l40 40H126Z" fill="#b387f0" opacity=".065"/>
    <path d="M708 514h52l27 53h-99Z" fill="#8bbdfc" opacity=".06"/>
    <path d="M109 304v35h3v-35Zm49 0v35h3v-35Z" fill="#ffc588" opacity=".33"/>
    <path d="M188 322h47v1h-47Zm20 22h23v1h-23Z" fill="#ffcf93" opacity=".6"/>
    <path d="M751 314h2v37h-2Zm33 0h2v37h-2Z" fill="#85ffe0" opacity=".4"/>
    <path d="M115 581h178v1H115Z" fill="#b991ec" opacity=".3"/>`);
  glow(133, 324, 65, "#ff812e", 0.85, 0.066);
  glow(224, 340, 42, "#ffbb67", 0.6, 0.035);
  glow(677, 320, 61, "#32edc4", 0.7, 0.035);
  glow(222, 533, 48, "#ac78fb", 0.65, 0.04);
  glow(738, 519, 58, "#739cd4", 0.6, 0.027);
  glow(593, 516, 48, "#ac94ed", 0.65, 0.027);
  glow(450, 207, 72, "#ffbd54", 0.6, 0.033);
  for (const [x, y] of [
    [374, 216],
    [522, 216],
    [589, 345],
    [814, 345],
    [268, 524],
    [423, 418],
    [475, 418],
  ])
    glow(x!, y!, 26, "#ffbf67", 1, 0.032);
  return `<g class="scenery-light" style="mix-blend-mode:screen;pointer-events:none">${items.join("")}</g>`;
}
