import type { ExportResult } from './types';

// Import cancellation must never trigger automation. Drop automation only processes
// newly imported IDs; it cannot re-clean old queue entries or previously verified copies.
export function autoCleanIds(
  enabled: boolean,
  fromDrop: boolean,
  cancelled: boolean,
  newIds: readonly string[],
): string[] {
  return enabled && fromDrop && !cancelled ? [...newIds] : [];
}

export function pendingIds(
  selected: ReadonlySet<string>,
  results: ReadonlyMap<string, ExportResult>,
): string[] {
  return [...selected].filter((id) => !verifiedOutput(results.get(id)));
}

export function verifiedOutput(result: ExportResult | undefined): string | null {
  return result?.status === 'verified' && result.encodedVerified && result.pixelsVerified
    ? result.output
    : null;
}
