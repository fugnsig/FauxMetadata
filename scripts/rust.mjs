import { spawnSync } from 'node:child_process';
import { buildEnv } from './env.mjs';
const result = spawnSync('cargo', process.argv.slice(2), {
  cwd: 'src-tauri',
  env: buildEnv(),
  stdio: 'inherit',
  shell: false,
});
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
