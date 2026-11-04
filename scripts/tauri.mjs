import { spawnSync } from 'node:child_process';
import { buildEnv } from './env.mjs';
import { resolve } from 'node:path';
const result = spawnSync(
  process.execPath,
  [resolve('node_modules/@tauri-apps/cli/tauri.js'), ...process.argv.slice(2)],
  { env: buildEnv(), stdio: 'inherit', shell: false },
);
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
