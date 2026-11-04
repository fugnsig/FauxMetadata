// One fail-fast entry point for local and hosted Windows checks.
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

const npm = process.env.npm_execpath;
if (!npm) throw new Error('Run this check with npm run verify.');
const commands = [
  ['format:check'],
  ['lint'],
  ['build'],
  ['test'],
  ['rust:fmt'],
  ['rust:check'],
  ['rust:test'],
  null,
  ['test:layout'],
];
for (const command of commands) {
  const args = command ? [npm, 'run', ...command] : [resolve('scripts/create-qa-fixtures.mjs')];
  const result = spawnSync(process.execPath, args, { stdio: 'inherit', shell: false });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
