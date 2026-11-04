import type { ExportResult, Field } from './types';
export function escapeHtml(value: string): string {
  return value.replace(
    /[&<>"']/g,
    (character) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character] ??
      character,
  );
}
export function bytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}
export function displayPath(path: string): string {
  if (path.startsWith('\\\\?\\UNC\\')) return `\\\\${path.slice(8)}`;
  return path.replace(/^\\\\\?\\/, '');
}
export function tagName(key: string): string {
  return key.split(':').at(-1) ?? key;
}
export function fieldLabel(field: Field): string {
  const tag = tagName(field.key);
  if (/workflow/i.test(tag)) return 'ComfyUI workflow';
  if (/^prompt$/i.test(tag)) return 'Generation prompt / graph';
  if (/parameters/i.test(tag)) return 'Generation parameters';
  return tag.replace(/([a-z])([A-Z])/g, '$1 $2');
}
export function prettyValue(value: string): string {
  if (value.length > 50000) return value;
  try {
    return JSON.stringify(JSON.parse(value) as unknown, null, 2);
  } catch {
    return value;
  }
}
export function filterFields(fields: Field[], category: string, query: string): Field[] {
  const search = query.trim().toLowerCase();
  return fields.filter(
    (field) =>
      (category === 'All' || category === field.category) &&
      (!search || `${field.key} ${field.value}`.toLowerCase().includes(search)),
  );
}
export function resultSummary(results: ExportResult[]): string {
  const verified = results.filter((result) => result.status === 'verified').length;
  const failed = results.filter((result) => result.status === 'error').length;
  const cancelled = results.filter((result) => result.status === 'cancelled').length;
  return [
    `${verified} verified`,
    failed ? `${failed} failed` : '',
    cancelled ? `${cancelled} cancelled` : '',
  ]
    .filter(Boolean)
    .join(' · ');
}
