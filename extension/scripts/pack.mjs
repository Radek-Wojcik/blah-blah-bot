// Zips target/extension/unpacked into target/extension/blah-blah-bot-extension-<version>.zip
// (the file attached to a release). Run `npm run build` first; `npm run pack` does both.
// A minimal zip writer (deflate, no dependencies); the entries are sorted and stamped with a
// fixed date so the same sources give the same archive.
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const src = path.resolve(here, "..", "..", "target", "extension", "unpacked");
if (!fs.existsSync(path.join(src, "manifest.json"))) {
  console.error("Nothing to pack: run `npm run build` first.");
  process.exit(1);
}
if (fs.existsSync(path.join(src, "test-hook.js"))) {
  console.error("The unpacked folder contains the test hook; rebuild without --test.");
  process.exit(1);
}
const manifest = JSON.parse(fs.readFileSync(path.join(src, "manifest.json"), "utf8"));
const outFile = path.resolve(src, "..", `blah-blah-bot-extension-${manifest.version}.zip`);

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

function list(dir, prefix = "") {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .flatMap((e) =>
      e.isDirectory()
        ? list(path.join(dir, e.name), `${prefix}${e.name}/`)
        : [{ name: `${prefix}${e.name}`, file: path.join(dir, e.name) }],
    )
    .sort((a, b) => (a.name < b.name ? -1 : 1));
}

const DOS_TIME = 0;
const DOS_DATE = (2026 - 1980) * 512 + 1 * 32 + 1; // 2026-01-01
const locals = [];
const centrals = [];
let offset = 0;
for (const { name, file } of list(src)) {
  const data = fs.readFileSync(file);
  const deflated = zlib.deflateRawSync(data, { level: 9 });
  const useDeflate = deflated.length < data.length;
  const body = useDeflate ? deflated : data;
  const nameBuf = Buffer.from(name, "utf8");
  const crc = crc32(data);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4); // version needed
  local.writeUInt16LE(0x0800, 6); // UTF-8 names
  local.writeUInt16LE(useDeflate ? 8 : 0, 8);
  local.writeUInt16LE(DOS_TIME, 10);
  local.writeUInt16LE(DOS_DATE, 12);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(body.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(nameBuf.length, 26);
  locals.push(local, nameBuf, body);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(0x0800, 8);
  central.writeUInt16LE(useDeflate ? 8 : 0, 10);
  central.writeUInt16LE(DOS_TIME, 12);
  central.writeUInt16LE(DOS_DATE, 14);
  central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(body.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(nameBuf.length, 28);
  central.writeUInt32LE(offset, 42);
  centrals.push(central, nameBuf);
  offset += local.length + nameBuf.length + body.length;
}
const centralBuf = Buffer.concat(centrals);
const end = Buffer.alloc(22);
end.writeUInt32LE(0x06054b50, 0);
end.writeUInt16LE(centrals.length / 2, 8);
end.writeUInt16LE(centrals.length / 2, 10);
end.writeUInt32LE(centralBuf.length, 12);
end.writeUInt32LE(offset, 16);
fs.writeFileSync(outFile, Buffer.concat([...locals, centralBuf, end]));
console.log(`${path.basename(outFile)} written (${centrals.length / 2} files)`);
