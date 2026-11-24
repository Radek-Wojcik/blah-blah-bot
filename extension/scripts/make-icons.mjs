// Generates icons/icon-{16,32,48,128}.png: a rounded blue square with three white sound bars.
// Run once (`node scripts/make-icons.mjs`); the PNG files are committed.
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(here, "..", "icons");
fs.mkdirSync(outDir, { recursive: true });

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

/** Coverage (0..1) of the icon shape at a point in unit coordinates. */
function shape(x, y) {
  const r = 0.22;
  const dx = Math.max(Math.abs(x - 0.5) - (0.5 - r), 0);
  const dy = Math.max(Math.abs(y - 0.5) - (0.5 - r), 0);
  if (dx * dx + dy * dy > r * r) return null; // outside the rounded square
  const bars = [
    [0.27, 0.4],
    [0.5, 0.64],
    [0.73, 0.5],
  ];
  for (const [cx, h] of bars) {
    const half = 0.075;
    const hh = h / 2;
    const ex = Math.max(Math.abs(x - cx) - (half - half), 0);
    const ey = Math.max(Math.abs(y - 0.5) - (hh - half), 0);
    if (ex * ex + ey * ey <= half * half) return "bar";
  }
  return "bg";
}

function png(size) {
  const ss = 4;
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let py = 0; py < size; py++) {
    raw[py * (size * 4 + 1)] = 0;
    for (let px = 0; px < size; px++) {
      let a = 0;
      let bar = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const s = shape((px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size);
          if (s) a += 1;
          if (s === "bar") bar += 1;
        }
      }
      const n = ss * ss;
      const t = a ? bar / a : 0;
      const o = py * (size * 4 + 1) + 1 + px * 4;
      raw[o] = Math.round(26 + (255 - 26) * t);
      raw[o + 1] = Math.round(115 + (255 - 115) * t);
      raw[o + 2] = Math.round(232 + (255 - 232) * t);
      raw[o + 3] = Math.round((255 * a) / n);
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

for (const size of [16, 32, 48, 128]) {
  fs.writeFileSync(path.join(outDir, `icon-${size}.png`), png(size));
}
console.log("icons written to", outDir);
