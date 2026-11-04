import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
export function buildEnv() {
  const env = { ...process.env };
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === 'path');
  const systemPath = pathKey ? env[pathKey] : '';
  for (const key of Object.keys(env)) if (key.toLowerCase() === 'path') delete env[key];
  env.Path = systemPath ?? '';
  const local = resolve('.tools/cargo');
  if (existsSync(local)) {
    env.CARGO_HOME = local;
    env.RUSTUP_HOME = resolve('.tools/rustup');
    env.Path = `${local}/bin;${env.Path}`;
  }
  return env;
}
