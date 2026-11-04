import { describe, expect, it } from 'vitest';
import { autoCleanIds, pendingIds, verifiedOutput } from './quick-clean';
import type { ExportResult } from './types';

const result = (status: ExportResult['status']): ExportResult => ({
  id: '1',
  status,
  output: 'C:\\Pictures\\Cleaned Images\\image_clean.png',
  error: null,
  removed: [],
  retained: [],
  after: [],
  removedChunks: [],
  bytes: 20,
  encodedVerified: status === 'verified',
  pixelsVerified: status === 'verified',
});
describe('Quick Clean transitions', () => {
  it('requires an opted-in, uncancelled drop with new images', () => {
    expect(autoCleanIds(false, true, false, ['1'])).toEqual([]);
    expect(autoCleanIds(true, false, false, ['1'])).toEqual([]);
    expect(autoCleanIds(true, true, true, ['1'])).toEqual([]);
    expect(autoCleanIds(true, true, false, [])).toEqual([]);
    expect(autoCleanIds(true, true, false, ['2', '3'])).toEqual(['2', '3']);
  });
  it('does not repeatedly clean verified queue entries; errors and cancellation can retry', () => {
    const results = new Map([
      ['1', result('verified')],
      ['2', result('cancelled')],
      ['3', result('error')],
    ]);
    expect(pendingIds(new Set(['1', '2', '3', '4']), results)).toEqual(['2', '3', '4']);
    expect(pendingIds(new Set(['1']), results)).toEqual([]);
    expect(
      pendingIds(
        new Set(['1']),
        new Map([['1', { ...result('verified'), pixelsVerified: false }]]),
      ),
    ).toEqual(['1']);
  });
  it('offers output actions only for completed verification', () => {
    expect(verifiedOutput(undefined)).toBeNull();
    expect(verifiedOutput(result('cancelled'))).toBeNull();
    expect(verifiedOutput(result('error'))).toBeNull();
    expect(verifiedOutput({ ...result('verified'), pixelsVerified: false })).toBeNull();
    expect(verifiedOutput({ ...result('verified'), encodedVerified: false })).toBeNull();
    expect(verifiedOutput(result('verified'))).toContain('_clean.png');
  });
});
