import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join, resolve, relative } from 'node:path';
const root = resolve('src-tauri/resources/exiftool');
const manifest = JSON.parse(readFileSync('docs/exiftool-manifest.json', 'utf8'));
function files(path) {
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isSymbolicLink()) throw new Error('Vendor symlinks are not allowed');
    const full = join(path, entry.name);
    return entry.isDirectory() ? files(full) : [full];
  });
}
const found = files(root);
if (found.length !== Object.keys(manifest.files).length)
  throw new Error('Bundled ExifTool file inventory changed');
for (const file of found) {
  const path = relative(root, file).replaceAll('\\', '/');
  const digest = createHash('sha256').update(readFileSync(file)).digest('hex');
  if (manifest.files[path] !== digest)
    throw new Error(`Bundled ExifTool checksum mismatch: ${path}`);
}
console.log(`Verified ${manifest.version} vendor inventory: ${found.length} files.`);
