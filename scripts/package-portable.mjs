import { createHash } from 'node:crypto';
import { cpSync, copyFileSync, mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
const { version } = JSON.parse(readFileSync('package.json', 'utf8'));
const output = resolve(`artifacts/FauxMetadata-${version}-portable`);
if (existsSync(output))
  throw new Error(`Output already exists: ${output}. Rename it before repackaging.`);
const verify = spawnSync(process.execPath, ['scripts/verify-vendor.mjs'], { stdio: 'inherit' });
if (verify.status !== 0) throw new Error('Vendor verification failed');
const icons = spawnSync(process.execPath, ['scripts/verify-icon.mjs'], { stdio: 'inherit' });
if (icons.status !== 0) throw new Error('Embedded Windows icon verification failed');
mkdirSync(output, { recursive: true });
copyFileSync('src-tauri/target/release/fauxmetadata.exe', join(output, 'FauxMetadata.exe'));
cpSync('src-tauri/resources/exiftool', join(output, 'resources/exiftool'), { recursive: true });
for (const name of ['README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.txt'])
  copyFileSync(name, join(output, name));
// Only public README artwork belongs in the portable package.
cpSync('docs/images', join(output, 'docs/images'), { recursive: true });
const setupName = `FauxMetadata_${version}_x64-setup.exe`;
const installer = resolve('artifacts', setupName);
copyFileSync(join('src-tauri/target/release/bundle/nsis', setupName), installer);
const checksums = [join(output, 'FauxMetadata.exe'), installer]
  .map((path) => `${createHash('sha256').update(readFileSync(path)).digest('hex')}  ${path}`)
  .join('\n');
writeFileSync('artifacts/SHA256SUMS.txt', checksums + '\n');
console.log(`Portable folder: ${output}\nInstaller: ${installer}`);
