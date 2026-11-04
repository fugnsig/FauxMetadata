// Verify embedded Windows resources, independently of Explorer's icon cache.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const executable = process.argv[2] ?? 'src-tauri/target/release/fauxmetadata.exe';
const exe = readFileSync(executable);
const ico = readFileSync('src-tauri/icons/icon.ico');
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const pe = exe.readUInt32LE(0x3c);
assert.equal(exe.readUInt32LE(pe), 0x4550, 'Expected a Windows PE executable');
const optional = pe + 24;
const directory = optional + (exe.readUInt16LE(optional) === 0x20b ? 112 : 96);
const sections = [];
let section = optional + exe.readUInt16LE(pe + 20);
for (let index = 0; index < exe.readUInt16LE(pe + 6); index++, section += 40) {
  sections.push({
    rva: exe.readUInt32LE(section + 12),
    size: Math.max(exe.readUInt32LE(section + 8), exe.readUInt32LE(section + 16)),
    raw: exe.readUInt32LE(section + 20),
  });
}
function offset(rva) {
  const found = sections.find((value) => rva >= value.rva && rva < value.rva + value.size);
  assert.ok(found, 'Resource RVA must map to a PE section');
  return rva - found.rva + found.raw;
}
const base = offset(exe.readUInt32LE(directory + 16));
const resources = [];
function walk(relative, path = []) {
  const at = base + relative;
  const count = exe.readUInt16LE(at + 12) + exe.readUInt16LE(at + 14);
  for (let index = 0; index < count; index++) {
    const entry = at + 16 + index * 8;
    const id = exe.readUInt32LE(entry);
    const next = exe.readUInt32LE(entry + 4);
    if (next & 0x80000000) walk(next & 0x7fffffff, [...path, id]);
    else {
      const data = base + next;
      const start = offset(exe.readUInt32LE(data));
      resources.push({
        path: [...path, id],
        bytes: exe.subarray(start, start + exe.readUInt32LE(data + 4)),
      });
    }
  }
}
walk(0);
const icons = resources.filter((value) => value.path[0] === 3);
const groups = resources.filter((value) => value.path[0] === 14);
const frames = [];
assert.equal(ico.readUInt16LE(2), 1, 'Expected an ICO file');
for (let index = 0; index < ico.readUInt16LE(4); index++) {
  const at = 6 + index * 16;
  const start = ico.readUInt32LE(at + 12);
  const bytes = ico.subarray(start, start + ico.readUInt32LE(at + 8));
  const frame = { width: ico[at] || 256, height: ico[at + 1] || 256, sha256: digest(bytes) };
  assert.ok(
    icons.some((value) => digest(value.bytes) === frame.sha256),
    `Missing or stale embedded ${frame.width}px icon. Rebuild native resources before packaging.`,
  );
  assert.ok(
    groups.some((group) => {
      for (let index = 0; index < group.bytes.readUInt16LE(4); index++) {
        const entry = 6 + index * 14;
        const width = group.bytes[entry] || 256;
        const height = group.bytes[entry + 1] || 256;
        const id = group.bytes.readUInt16LE(entry + 12);
        if (
          width === frame.width &&
          height === frame.height &&
          icons.some((icon) => icon.path[1] === id && digest(icon.bytes) === frame.sha256)
        )
          return true;
      }
      return false;
    }),
    `Windows icon group does not reference the ${frame.width}px artwork`,
  );
  frames.push(frame);
}
assert.deepEqual(
  frames.map((value) => value.width).sort((a, b) => a - b),
  [16, 24, 32, 48, 64, 128, 256],
);
const report = {
  executable,
  sha256: digest(exe),
  embeddedIconResources: icons.length,
  allFramesMatch: true,
  frames,
};
mkdirSync('artifacts', { recursive: true });
writeFileSync('artifacts/icon-verification.json', JSON.stringify(report, null, 2) + '\n');
console.log(`Verified all ${frames.length} supplied ICO frames in Windows executable resources.`);
