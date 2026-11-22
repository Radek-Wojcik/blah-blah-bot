// Assembles the loadable extension folder from the compiled scripts (dist/) and the static
// files. `--test` adds the test hook to the offscreen document and writes to
// target/extension/test-unpacked instead of target/extension/unpacked.
//
// The packaged build never contains test-hook.js.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const test = process.argv.includes("--test");
const out = path.resolve(root, "..", "target", "extension", test ? "test-unpacked" : "unpacked");

fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });

const dist = path.join(root, "dist");
for (const name of fs.readdirSync(dist)) {
  if (!name.endsWith(".js")) continue;
  if (name === "test-hook.js" && !test) continue;
  fs.copyFileSync(path.join(dist, name), path.join(out, name));
}
for (const name of ["manifest.json", "popup.html"]) {
  fs.copyFileSync(path.join(root, name), path.join(out, name));
}
let offscreen = fs.readFileSync(path.join(root, "offscreen.html"), "utf8");
if (test) {
  offscreen = offscreen.replace(
    '<script type="module" src="offscreen.js"></script>',
    '<script type="module" src="offscreen.js"></script>\n    <script type="module" src="test-hook.js"></script>',
  );
}
fs.writeFileSync(path.join(out, "offscreen.html"), offscreen);
fs.cpSync(path.join(root, "icons"), path.join(out, "icons"), { recursive: true });

const manifest = JSON.parse(fs.readFileSync(path.join(out, "manifest.json"), "utf8"));
console.log(`${test ? "test " : ""}extension ${manifest.version} written to ${out}`);
