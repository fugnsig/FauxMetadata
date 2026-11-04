import { describe, expect, it } from 'vitest';
import { displayPath, escapeHtml, filterFields, prettyValue, resultSummary } from './format';
import type { Field, ExportResult } from './types';
describe('untrusted metadata presentation', () => {
  it('fuzzes 10,000 Unicode and markup strings without creating HTML delimiters', () => {
    let seed = 20261104;
    const random = (): number => {
      seed ^= seed << 13;
      seed ^= seed >>> 17;
      seed ^= seed << 5;
      return seed >>> 0;
    };
    const alphabet = [
      '<',
      '>',
      '&',
      '"',
      "'",
      '\\',
      '\n',
      '\t',
      '花',
      '😀',
      '\u202e',
      '\0',
      'a',
      'amp;',
      '</script>',
    ];
    const entities: Record<string, string> = { amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" };
    for (let index = 0; index < 10000; index++) {
      const length = random() % 200;
      const value = Array.from({ length }, () => alphabet[random() % alphabet.length] ?? '').join(
        '',
      );
      const escaped = escapeHtml(value);
      expect(escaped).not.toMatch(/[<>"']/);
      expect(
        escaped.replace(/&(amp|lt|gt|quot|#39);/g, (_, entity: string) => entities[entity] ?? ''),
      ).toBe(value);
    }
  });
  it('escapes metadata rather than creating HTML or handlers', () => {
    expect(escapeHtml('<img src=x onerror="alert(1)"> & \'')).toBe(
      '&lt;img src=x onerror=&quot;alert(1)&quot;&gt; &amp; &#39;',
    );
  });
  it('keeps malformed workflow data visible and pretty-prints valid graphs', () => {
    expect(prettyValue('{broken')).toBe('{broken');
    expect(prettyValue('{"nodes":[]}')).toBe('{\n  "nodes": []\n}');
  });
  it('filters by category and case-insensitive field/value search together', () => {
    const fields: Field[] = [
      {
        key: 'PNG:Prompt',
        value: 'Private forest prompt',
        category: 'Generation',
        retained: false,
      },
      { key: 'IFD0:Software', value: 'Forest editor', category: 'EXIF', retained: false },
    ];
    expect(filterFields(fields, 'Generation', ' FOREST ')).toEqual([fields[0]]);
    expect(filterFields(fields, 'All', 'software')).toEqual([fields[1]]);
  });
  it('never counts failed or cancelled results as verified', () => {
    const results = ['verified', 'error', 'cancelled'].map(
      (status) => ({ status }) as ExportResult,
    );
    expect(resultSummary(results)).toBe('1 verified · 1 failed · 1 cancelled');
  });
  it('displays extended Windows and UNC paths without corrupting names', () => {
    expect(displayPath('\\\\?\\C:\\photos\\花.png')).toBe('C:\\photos\\花.png');
    expect(displayPath('\\\\server\\photos')).toBe('\\\\server\\photos');
    expect(displayPath('\\\\?\\UNC\\server\\photos')).toBe('\\\\server\\photos');
  });
});
