import { describe, expect, it } from 'vitest';
import type { ExportResult } from './types';
import { batchOutcome, outputDirectory, phaseLabel } from './ui-state';

function result(status: ExportResult['status'], id = '1'): ExportResult {
  return {
    id,
    status,
    output: status === 'verified' ? 'C:\\Pictures\\Cleaned Images\\花_clean.png' : null,
    error: null,
    removed: [],
    retained: [],
    after: [],
    removedChunks: [],
    bytes: 1,
    encodedVerified: status === 'verified',
    pixelsVerified: status === 'verified',
  };
}
describe('honest desktop feedback', () => {
  it('shows the real processing phase without claiming early verification', () => {
    expect(phaseLabel('staging')).toBe('Preparing safe copy…');
    expect(phaseLabel('stripping')).toBe('Removing metadata…');
    expect(phaseLabel('verifying')).toBe('Verifying clean copy…');
    expect(phaseLabel('publishing')).toBe('Saving verified copy…');
  });
  it('reports mixed results and points at the verified output, not a newly selected destination', () => {
    const outcome = batchOutcome([result('error'), result('verified', '2'), result('cancelled')]);
    expect(outcome.label).toBe('1 clean copy verified · 1 failed · 1 cancelled');
    expect(outcome.tone).toBe('warning');
    expect(outcome.folder).toBe('C:\\Pictures\\Cleaned Images');
    expect(outcome.firstVerifiedId).toBe('2');
  });
  it('never offers output actions or green success for incomplete verification', () => {
    const unverified = { ...result('verified'), pixelsVerified: false };
    expect(batchOutcome([unverified])).toEqual({
      label: '0 clean copies verified · 1 failed',
      tone: 'warning',
      folder: null,
      firstVerifiedId: null,
    });
    expect(batchOutcome([result('cancelled')]).tone).toBe('neutral');
    expect(batchOutcome([]).folder).toBeNull();
  });
  it('preserves Unicode, UNC and drive-root output directory paths', () => {
    expect(outputDirectory('\\\\?\\UNC\\server\\Photos 花\\image.png')).toBe(
      '\\\\server\\Photos 花',
    );
    expect(outputDirectory('\\\\?\\C:\\image.png')).toBe('C:\\');
    expect(outputDirectory('D:/Photos 花/image.png')).toBe('D:/Photos 花');
  });
});
