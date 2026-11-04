import type { ExportResult } from './types';
import { displayPath } from './format';
import { verifiedOutput } from './quick-clean';

export function phaseLabel(phase: string): string {
  const labels: Record<string, string> = {
    importing: 'Inspecting images…',
    staging: 'Preparing safe copy…',
    stripping: 'Removing metadata…',
    verifying: 'Verifying clean copy…',
    publishing: 'Saving verified copy…',
    complete: 'Processing images…',
  };
  return labels[phase] ?? 'Preparing…';
}

export function outputDirectory(output: string): string {
  const path = displayPath(output);
  const slash = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  if (slash === 2 && path[1] === ':') return path.slice(0, 3);
  return slash > 0 ? path.slice(0, slash) : path;
}

export function batchOutcome(results: readonly ExportResult[]): {
  label: string;
  tone: 'verified' | 'warning' | 'neutral';
  folder: string | null;
  firstVerifiedId: string | null;
} {
  const verified = results.filter((result) => verifiedOutput(result));
  const cancelled = results.filter((result) => result.status === 'cancelled').length;
  const failed = results.length - verified.length - cancelled;
  const parts = [`${verified.length} clean ${verified.length === 1 ? 'copy' : 'copies'} verified`];
  if (failed) parts.push(`${failed} failed`);
  if (cancelled) parts.push(`${cancelled} cancelled`);
  const first = verified[0];
  return {
    label: parts.join(' · '),
    tone: failed ? 'warning' : verified.length ? 'verified' : 'neutral',
    folder: first?.output ? outputDirectory(first.output) : null,
    firstVerifiedId: first?.id ?? null,
  };
}
