import './styles.css';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import type {
  ExportResult,
  Health,
  ImportIssue,
  ImportResult,
  Inspection,
  Progress,
  Preferences,
} from './types';
import { autoCleanIds, pendingIds, verifiedOutput } from './quick-clean';
import { batchOutcome, phaseLabel } from './ui-state';
import {
  bytes,
  displayPath,
  escapeHtml as esc,
  fieldLabel,
  filterFields,
  prettyValue,
} from './format';

const appIcon = new URL('./assets/app-icon.png', import.meta.url).href;
const icons = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z"/>',
  arrow: '<path d="M5 12h14m-5-5 5 5-5 5"/>',
  shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9S4 17 4 12V6Z"/><path d="m8 12 3 3 5-6"/>',
  image:
    '<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8" cy="8" r="1"/><path d="m3 17 5-5 4 4 4-6 5 7"/>',
  upload: '<path d="M12 16V3m-5 5 5-5 5 5M4 16v5h16v-5"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  cross: '<path d="m6 6 12 12M6 18 18 6"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  lock: '<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V6a4 4 0 0 1 8 0v4"/>',
  spark: '<path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7v1"/>',
};
type Icon = keyof typeof icons;
const icon = (name: Icon, className = ''): string =>
  `<svg class="icon ${className}" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name]}</svg>`;
const root = document.querySelector<HTMLDivElement>('#app');
if (!root) throw new Error('Application root missing');
const app = root;
let images: Inspection[] = [];
let activeId: string | null = null;
const selected = new Set<string>();
const results = new Map<string, ExportResult>();
let destination = '';
let preferences: Preferences | null = null;
let advanced = false;
let queueExpanded = false;
let optionsOpen = false;
let lastView = '';
let feedback = '';
let ready = false;
let engineFailed = false;
let health: Health | null = null;
let category = 'All';
let query = '';
let mode: 'source' | 'result' = 'source';
let busy: 'import' | 'export' | null = null;
let progress = { completed: 0, total: 0, phase: '', id: '' };
let job = '';
let restoreOperationFocus = false;
let message = '';
let issues: ImportIssue[] = [];
let lastResults: ExportResult[] = [];
let showPolicy = false;
let cancelling = false;
let dialogOpen = false;

function activeImage(): Inspection | undefined {
  return images.find((image) => image.id === activeId);
}
function button(
  action: string,
  label: string,
  glyph: Icon,
  className = '',
  disabled = false,
): string {
  return `<button type="button" data-action="${action}" class="button ${className}" aria-label="${esc(label)}" title="${esc(label)}" ${disabled ? 'disabled' : ''}>${icon(glyph)}<span>${esc(label)}</span></button>`;
}
function announce(text: string): void {
  const live = document.querySelector('#live');
  if (live) live.textContent = text;
}
// The native caption is hidden; these mirror Windows' minimise, maximise/restore and close buttons.
let windowMaximised = false;
function windowControls(): string {
  if (!isTauri()) return '';
  const toggle = windowMaximised ? 'Restore' : 'Maximise';
  return `<div class="window-controls"><button type="button" id="window-minimise" class="window-control" data-window="minimise" aria-label="Minimise" title="Minimise"><span aria-hidden="true">&#xE921;</span></button><button type="button" id="window-maximise" class="window-control" data-window="maximise" aria-label="${toggle}" title="${toggle}"><span aria-hidden="true">${windowMaximised ? '&#xE923;' : '&#xE922;'}</span></button><button type="button" id="window-close" class="window-control close" data-window="close" aria-label="Close" title="Close"><span aria-hidden="true">&#xE8BB;</span></button></div>`;
}
async function syncWindowState(): Promise<void> {
  try {
    const maximised = await getCurrentWebviewWindow().isMaximized();
    if (maximised === windowMaximised) return;
    windowMaximised = maximised;
    const button = app.querySelector<HTMLButtonElement>('#window-maximise');
    if (!button) return;
    const label = maximised ? 'Restore' : 'Maximise';
    button.setAttribute('aria-label', label);
    button.title = label;
    button.innerHTML = `<span aria-hidden="true">${maximised ? '&#xE923;' : '&#xE922;'}</span>`;
  } catch {
    // Window state is cosmetic; the controls still work if it cannot be read.
  }
}
function windowCommand(name: string): void {
  const current = getCurrentWebviewWindow();
  const command =
    name === 'minimise'
      ? current.minimize()
      : name === 'maximise'
        ? current.toggleMaximize()
        : current.close();
  void command.catch((error: unknown) => {
    message = String(error);
    render();
  });
}
function render(): void {
  const focused = document.activeElement as HTMLElement | null;
  const focusId = focused?.id;
  const focusAction = focused?.dataset.action;
  const focusImage = focused?.dataset.image;
  const focusSelect = focused?.dataset.select;
  const image = activeImage();
  const view = `${activeId}:${advanced}:${mode}`;
  const sameView = view === lastView;
  const inspectorScroll = app.querySelector('.inspector')?.scrollTop ?? 0;
  const queueScroll = app.querySelector('.queue')?.scrollTop ?? 0;
  const expanded = sameView
    ? Array.from(app.querySelectorAll<HTMLDetailsElement>('details[data-expand][open]')).map(
        (el) => el.dataset.expand,
      )
    : [];
  lastView = view;
  const blocked = !!busy || dialogOpen || !ready;
  app.innerHTML = `
    <header class="topbar" data-tauri-drag-region><a class="brand" href="#" data-action="home" aria-label="FauxMetadata home"><img class="brand-icon" src="${appIcon}" alt="" width="56" height="56"><span class="brand-name"><span class="brand-prefix">Faux</span>Metadata</span></a><div class="topbar-drag" data-tauri-drag-region></div><div class="topbar-right"><button type="button" class="view-switch" data-action="advanced" aria-pressed="${advanced}">${advanced ? 'Quick Clean' : 'Advanced'}</button><button type="button" class="icon-button" data-action="policy" title="About cleaning" aria-label="About cleaning" aria-expanded="${showPolicy}">${icon('info')}</button></div>${windowControls()}</header>
    <main class="workspace ${advanced ? 'advanced-mode' : 'quick-mode'} ${images.length ? 'with-images' : 'is-empty'}">
      <div class="workspace-toolbar"><div><h1>${advanced ? 'Metadata & verification' : 'Quick Clean'}</h1>${advanced ? '<p>Original fields and verified output reports.</p>' : ''}</div>${images.length ? `<div class="toolbar-actions">${button('add', 'Add images', 'plus', 'secondary', blocked)}${button('folder', 'Add folder', 'folder', 'quiet', blocked)}</div>` : ''}</div>
      ${!isTauri() ? '<div class="notice error" role="alert">Open the Windows application to clean images. Browser previews do not perform file operations.</div>' : ''}
      ${message ? `<div class="notice error" role="alert">${icon('info')}<span>${esc(message)}</span><button type="button" class="icon-button" data-action="dismiss" aria-label="Dismiss error">${icon('cross')}</button></div>` : ''}
      ${feedback ? `<div class="notice success" role="status">${icon('check')}<span>${esc(feedback)}</span><button type="button" class="icon-button" data-action="dismiss-feedback" aria-label="Dismiss confirmation">${icon('cross')}</button></div>` : ''}
      ${showPolicy ? policy() : ''}
      <div class="workbench ${images.length ? 'has-images' : ''}" aria-busy="${busy !== null}">
        ${images.length ? queuePanel() : ''}
        <section class="inspector ${mode === 'result' ? 'report-view' : ''}" aria-label="${advanced ? 'Metadata inspector' : 'Image preview and cleaning result'}">${image ? (advanced ? inspector(image) : quickPreview(image)) : emptyState()}</section>
      </div>
      ${issues.length ? `<details class="issues"><summary>${icon('info')} ${issues.length} skipped file${issues.length === 1 ? '' : 's'} / import notice${issues.length === 1 ? '' : 's'}</summary><ul>${issues.map((issue) => `<li><strong>${esc(displayPath(issue.path))}</strong><p>${esc(issue.message)}</p></li>`).join('')}</ul></details>` : ''}
      ${lastResults.length ? batchReport() : ''}
      ${exportBar()}
      <footer class="footer"><span>Originals stay untouched</span><span>${health ? `Offline · ExifTool ${esc(health.exiftool)}` : engineFailed ? 'Engine unavailable' : 'Checking metadata engine…'}</span></footer>
    </main><div id="drop-overlay" class="drop-overlay" hidden><div>${icon('upload')}<strong>${preferences?.autoCleanOnDrop ? 'Drop to clean and verify' : 'Drop to add images'}</strong><span>PNG, JPEG, WebP and folders</span></div></div>`;
  bind();
  const candidates = Array.from(app.querySelectorAll<HTMLElement>('button, input, select, a'));
  const previous = candidates.find(
    (el) =>
      (focusId && el.id === focusId) ||
      (focusAction && el.dataset.action === focusAction) ||
      (focusImage && el.dataset.image === focusImage) ||
      (focusSelect && el.dataset.select === focusSelect),
  );
  app.querySelectorAll<HTMLDetailsElement>('details[data-expand]').forEach((el) => {
    if (expanded.includes(el.dataset.expand)) el.open = true;
  });
  if (sameView) {
    const inspector = app.querySelector('.inspector');
    const queue = app.querySelector('.queue');
    if (inspector) inspector.scrollTop = inspectorScroll;
    if (queue) queue.scrollTop = queueScroll;
  }
  if (previous && !(previous as HTMLButtonElement).disabled)
    previous.focus({ preventScroll: true });
  else if (focusAction === 'export' && busy) focusActionButton('cancel');
  else if (focusAction === 'cancel' && !busy)
    focusActionButton(
      lastResults.some((result) => verifiedOutput(result)) ? 'open-last-output' : 'export',
    );
  else if ((focusAction === 'clear' || focusAction === 'remove') && !images.length)
    focusActionButton('add');
}

function focusActionButton(name: string): void {
  app
    .querySelector<HTMLButtonElement>(`[data-action="${name}"]:not(:disabled)`)
    ?.focus({ preventScroll: true });
}
function queuePanel(): string {
  const locked = !!busy || dialogOpen;
  return `<aside class="queue-panel ${queueExpanded ? 'queue-expanded' : ''}" aria-label="Image queue"><button type="button" class="compact-queue-toggle" data-action="toggle-queue" aria-expanded="${queueExpanded}" aria-controls="image-queue-items">${icon('image')}<span>Images <span class="count">${images.length}</span> · ${selected.size} selected</span>${icon('arrow')}</button><div class="panel-heading"><h2>Images <span class="count">${images.length}</span></h2><button type="button" class="text-button" data-action="clear" ${locked ? 'disabled' : ''}>Clear queue</button></div><div class="queue-tools"><label class="checkbox-label"><input type="checkbox" id="select-all" aria-label="Select all images" ${selected.size === images.length ? 'checked' : ''} ${locked ? 'disabled' : ''}>${selected.size} of ${images.length} selected</label></div><div class="queue" id="image-queue-items" role="list" aria-label="Imported images">${images.map(queueItem).join('')}</div></aside>`;
}
function batchReport(): string {
  const outcome = batchOutcome(lastResults);
  return `<section class="batch-summary ${outcome.tone}" role="status" aria-label="Batch results">${icon(outcome.tone === 'verified' ? 'check' : 'info')}<div class="batch-copy"><strong>${esc(outcome.label)}</strong><span class="batch-location" title="${esc(outcome.folder ?? '')}">${esc(outcome.folder ?? 'No verified copies were saved.')}</span></div><div class="batch-actions">${outcome.firstVerifiedId ? button('open-last-output', 'Open Folder', 'folder', 'secondary', !!busy) : ''}<button type="button" class="text-button" data-action="clear" ${busy ? 'disabled' : ''}>New batch</button></div></section>`;
}
function exportBar(): string {
  const count = advanced ? selected.size : pendingIds(selected, results).length;
  const locked = !!busy || dialogOpen;
  const hint = engineFailed
    ? 'Engine unavailable'
    : !ready
      ? 'Checking engine…'
      : !images.length
        ? 'Add images to begin'
        : count
          ? `${count} image${count === 1 ? '' : 's'} ready`
          : !selected.size
            ? 'No images selected'
            : 'Selected copies verified';
  return `<section class="export-bar" aria-label="Output folder and cleaning actions"><div class="destination"><div class="destination-heading"><span>Save cleaned copies to</span>${preferences?.isDefault ? '<span class="default-label">Default</span>' : ''}<button type="button" class="text-button" data-action="destination" ${locked ? 'disabled' : ''}>Change…</button><details class="destination-options" ${optionsOpen ? 'open' : ''}><summary aria-label="Output options">Options</summary><div class="options-panel"><label class="checkbox-label"><input id="auto-clean" type="checkbox" ${preferences?.autoCleanOnDrop ? 'checked' : ''} ${locked || !ready ? 'disabled' : ''}>Auto-clean on drop</label><p>Clean newly dropped images after inspection.</p><button type="button" class="text-button" data-action="reset-destination" ${locked ? 'disabled' : ''}>Reset to Default</button></div></details></div><div class="destination-path" title="${esc(destination)}">${destination ? esc(destination) : 'Output folder unavailable - choose a folder or reset'}</div></div><div class="export-actions">${busy ? `<div class="progress-text"><strong>${cancelling ? 'Cancelling…' : esc(phaseLabel(progress.phase))}</strong><span>${progress.completed} of ${progress.total} images</span></div>${button('cancel', cancelling ? 'Cancelling…' : 'Cancel', 'cross', 'secondary', cancelling)}` : `<span class="export-hint">${esc(hint)}</span>${button('export', 'Clean' + (count > 1 ? ` ${count} images` : ''), 'spark', 'primary', !count || !destination || !health || !ready || dialogOpen)}`}</div>${busy ? `<progress class="batch-progress" max="${Math.max(progress.total, 1)}" value="${progress.completed}" aria-label="${esc(phaseLabel(progress.phase))}"></progress>` : ''}</section>`;
}
function outputActions(image: Inspection, compact = false): string {
  const output = verifiedOutput(results.get(image.id));
  return output
    ? `<div class="output-actions">${compact ? '' : button('open-output', 'Open Folder', 'folder', 'secondary', !!busy)}${button('copy-output', 'Copy Path', 'arrow', 'quiet', !!busy)}</div>`
    : '';
}
function quickPreview(image: Inspection): string {
  const result = results.get(image.id);
  const output = verifiedOutput(result);
  const running = busy === 'export' && progress.id === image.id && progress.phase !== 'complete';
  const state = running
    ? phaseLabel(progress.phase)
    : output
      ? 'Clean copy verified'
      : result
        ? result.status === 'cancelled'
          ? 'Cancelled'
          : 'Cleaning failed'
        : 'Ready to clean';
  const generation = image.fields.filter((field) => field.category === 'Generation').length;
  return `<div class="quick-preview"><div class="preview-heading"><div><h2 title="${esc(image.name)}">${esc(image.name)}</h2><p>${image.format} · ${image.width.toLocaleString()} × ${image.height.toLocaleString()} · ${bytes(image.bytes)}</p></div><button type="button" class="icon-button" data-action="remove" aria-label="Remove ${esc(image.name)} from queue" title="Remove from queue" ${busy ? 'disabled' : ''}>${icon('cross')}</button></div><div class="quick-image"><img src="${image.preview}" alt="Preview of ${esc(image.name)}"></div><div class="quick-result"><div class="quick-status ${output ? 'verified' : result?.status === 'error' ? 'error' : ''}">${icon(output ? 'check' : result ? 'info' : 'image')}<strong>${esc(state)}</strong></div>${output ? `<p class="output-location" title="${esc(displayPath(output))}">${esc(displayPath(output))}</p>${outputActions(image, true)}` : result ? `<p class="item-error">${esc(result.error ?? 'No verified output was published.')}</p>` : `<p>${generation ? 'Workflow and generation data detected.' : 'Metadata inspected.'} ${image.fields.filter((field) => !field.retained).length} fields marked for removal.</p>`}<button type="button" class="text-button" data-action="advanced">${result ? 'View verification report' : 'Inspect metadata'} ${icon('arrow')}</button></div></div>`;
}
function emptyState(): string {
  return `<div class="empty-state"><div class="drop-target">${icon('upload')}<h2>Add images to clean</h2><p>Drop images or a folder here to remove embedded metadata.</p><div class="empty-actions">${button('add', 'Browse Images', 'plus', 'secondary', !!busy || !ready || !isTauri())}${button('folder', 'Add Folder', 'folder', 'quiet', !!busy || !ready || !isTauri())}</div><span class="supported-formats">PNG, JPEG and WebP · ComfyUI workflows, prompts, EXIF & GPS</span></div></div>`;
}
function queueItem(image: Inspection): string {
  const result = results.get(image.id);
  const generation = image.fields.some((field) => field.category === 'Generation');
  const running = busy === 'export' && progress.id === image.id && progress.phase !== 'complete';
  const output = verifiedOutput(result);
  const state = running
    ? phaseLabel(progress.phase)
    : output
      ? 'Verified copy'
      : result
        ? result.status === 'cancelled'
          ? 'Cancelled'
          : 'Failed'
        : generation
          ? 'Ready · Generation data'
          : 'Ready';
  return `<div class="queue-item ${image.id === activeId ? 'active' : ''}" role="listitem"><label class="queue-check"><input type="checkbox" data-select="${image.id}" aria-label="Select ${esc(image.name)} for cleaning" ${selected.has(image.id) ? 'checked' : ''} ${busy ? 'disabled' : ''}></label><button type="button" class="image-row" data-image="${image.id}" aria-current="${image.id === activeId ? 'true' : 'false'}"><img class="thumbnail" src="${image.preview}" alt=""><span class="image-info"><strong title="${esc(image.name)}">${esc(image.name)}</strong><span>${image.format} · ${bytes(image.bytes)}</span><span class="row-status ${output ? 'verified' : result ? (result.status === 'cancelled' ? 'cancelled' : 'error') : ''}" title="${esc(result?.error ?? '')}">${running ? '<span class="spinner" aria-hidden="true"></span>' : output ? icon('check') : result ? icon('info') : ''}${esc(state)}</span></span></button></div>`;
}
function inspector(image: Inspection): string {
  const result = results.get(image.id);
  const isResult = mode === 'result' && result;
  const fields = isResult ? result.after : image.fields;
  const visible = filterFields(fields, category, query);
  const categories = ['All', ...new Set(fields.map((field) => field.category))];
  const removable = image.fields.filter((field) => !field.retained).length;
  const generation = image.fields.filter((field) => field.category === 'Generation').length;
  return `<div class="inspector-heading"><div><div class="eyebrow">${isResult ? 'Verification report' : 'Original metadata'}</div><h2 title="${esc(image.name)}">${esc(image.name)}</h2><div class="source-path" title="${esc(displayPath(image.path))}">${esc(displayPath(image.path))}</div></div><button class="icon-button" data-action="remove" aria-label="Remove selected image from queue" title="Remove from queue" ${busy ? 'disabled' : ''}>${icon('cross')}</button></div>
    <div class="image-overview"><div class="preview"><img src="${image.preview}" alt="Preview of ${esc(image.name)}"></div><div class="image-facts"><span class="format-pill">${image.format}</span><h3>${image.width.toLocaleString()} × ${image.height.toLocaleString()}</h3><p>${bytes(image.bytes)} original</p><div class="fact-rule"></div><div class="fact ${generation ? 'generation' : ''}">${icon(generation ? 'spark' : 'info')}<span>${generation ? `${generation} generation field${generation === 1 ? '' : 's'}` : 'No recognised generation fields'}</span></div><div class="fact">${icon('shield')}<span>${removable} fields marked for removal</span></div><span class="small-note">Colour profiles and display orientation are retained and checked.</span></div></div>
    <div class="inspector-tabs" role="group" aria-label="Metadata view"><button data-mode="source" class="${mode === 'source' ? 'active' : ''}" aria-pressed="${mode === 'source'}">Original metadata <span>${image.fields.length}</span></button><button data-mode="result" class="${mode === 'result' ? 'active' : ''}" aria-pressed="${mode === 'result'}" ${!result ? 'disabled' : ''}>Export report ${result ? icon(verifiedOutput(result) ? 'check' : 'info') : ''}</button></div>
    ${isResult ? resultReport(result) + outputActions(image) : `<div class="policy-note">${icon('info')} Every export is reinspected. Unexpected residuals or appearance changes block the copy.</div>`}
    ${!isResult || verifiedOutput(result) ? `<div class="metadata-toolbar"><label class="search">${icon('search')}<input id="metadata-search" type="search" placeholder="Search metadata…" aria-label="Search metadata fields and values" value="${esc(query)}"></label><label class="sr-only" for="category-filter">Metadata category</label><select id="category-filter">${categories.map((item) => `<option ${category === item ? 'selected' : ''}>${esc(item)}</option>`).join('')}</select></div><div class="metadata-list" id="metadata-list">${metadataFields(visible, !!isResult)}</div>` : ''}`;
}
function metadataFields(fields: Inspection['fields'], output: boolean): string {
  if (!fields.length)
    return '<div class="no-fields">No fields in this view.<span>Structural image data is still present and required.</span></div>';
  return fields
    .map(
      (field) =>
        `<details class="metadata-field" data-expand="${esc(field.key)}"><summary><span class="field-symbol ${field.category === 'Generation' ? 'generation' : ''}">${icon(field.category === 'Generation' ? 'spark' : field.retained ? 'shield' : 'info')}</span><span class="field-title"><strong>${esc(fieldLabel(field))}</strong><span>${esc(field.key)}</span></span><span class="field-category">${esc(field.category)}</span><span class="tag ${field.retained || output ? 'keep' : 'remove'}">${field.retained || output ? 'Keep' : 'Remove'}</span></summary><div class="field-value"><pre>${esc(prettyValue(field.value))}</pre></div></details>`,
    )
    .join('');
}
function resultReport(result: ExportResult): string {
  if (!verifiedOutput(result))
    return `<div class="report error"><strong>${icon('info')} ${result.status === 'cancelled' ? 'Export cancelled' : 'Copy was not published'}</strong><p>${esc(result.error ?? 'Output verification did not complete. No clean copy is confirmed.')}</p><span>Your original is untouched. You can retry this image.</span></div>`;
  return `<div class="report verified"><strong>${icon('shield')} Sanitised copy verified</strong><div class="report-metrics"><span>${result.removed.length} fields removed</span><span>${bytes(result.bytes)} exported</span><span>Encoded data + pixels match</span></div><label class="eyebrow" for="output-path">Saved copy</label><input class="output-path" id="output-path" readonly value="${esc(displayPath(result.output ?? ''))}" aria-label="Saved output file path"><details class="diff" data-expand="removed-fields"><summary>See exactly what was removed</summary>${result.removed.length ? `<ul>${result.removed.map((field) => `<li><code>${esc(field.key)}</code><details><summary>Removed value</summary><pre>${esc(prettyValue(field.value))}</pre></details></li>`).join('')}</ul>` : '<p>No removable ExifTool fields were found in the original.</p>'}${result.removedChunks.length ? `<p>Removed container regions: ${result.removedChunks.map(esc).join(', ')}</p>` : ''}</details><p class="verification-scope">Verified against recognised metadata and container rules. ICC profile contents, image pixels, filenames and filesystem data are outside the privacy guarantee.</p></div>`;
}
function policy(): string {
  return `<aside class="policy-card"><div><h2>About cleaning</h2><p class="policy-version">FauxMetadata${health?.version ? ' ' + esc(health.version) : ''} · ExifTool ${esc(health?.exiftool ?? 'checking…')} · Entirely offline</p><p>FauxMetadata removes workflow and generation data, EXIF camera information, GPS, XMP, IPTC, software tags, PNG text and unapproved container regions. It writes a new file and never edits the original.</p><p><strong>Appearance comes first.</strong> ICC profiles, PNG colour and transparency data, JPEG rendering markers and display orientation are retained. The app compares encoded image data, decoded pixels and retained metadata before publishing.</p><p>Verification covers supported PNG, JPEG and WebP containers. Retained ICC profiles may contain descriptive information. It does not detect steganography or redact information visible in the picture. Output filenames and filesystem timestamps are not anonymised. Animated PNG/WebP, HDR, rotated WebP metadata and multi-picture JPEG are rejected.</p><p>Limits: 128 MiB per file, 40 megapixels, 500 queued images. Cancellation keeps finished exports and discards unfinished copies. The Windows app has no cloud services, telemetry, or network functionality.</p></div><button class="icon-button" data-action="policy" aria-label="Close policy">${icon('cross')}</button></aside>`;
}
function refreshFields(): void {
  const image = activeImage();
  if (!image) return;
  const fields = mode === 'result' ? (results.get(image.id)?.after ?? []) : image.fields;
  const list = document.querySelector('#metadata-list');
  if (list)
    list.innerHTML = metadataFields(filterFields(fields, category, query), mode === 'result');
}
function applyPreferences(value: Preferences): void {
  preferences = value;
  destination = value.destination ?? '';
  message = value.error ?? '';
}
function bind(): void {
  app
    .querySelector<HTMLDetailsElement>('.destination-options')
    ?.addEventListener('toggle', (event) => {
      const details = event.currentTarget as HTMLDetailsElement;
      if (details.isConnected) optionsOpen = details.open;
    });
  app.querySelector<HTMLInputElement>('#auto-clean')?.addEventListener('change', (event) => {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    dialogOpen = true;
    render();
    void invoke<Preferences>('set_auto_clean', { enabled })
      .then(applyPreferences)
      .catch((error: unknown) => {
        message = String(error);
      })
      .finally(() => {
        dialogOpen = false;
        render();
        app.querySelector<HTMLInputElement>('#auto-clean')?.focus();
      });
  });
  app
    .querySelectorAll<HTMLButtonElement>('[data-window]')
    .forEach((element) =>
      element.addEventListener('click', () => windowCommand(element.dataset.window ?? '')),
    );
  app.querySelectorAll<HTMLButtonElement>('[data-action]').forEach((element) =>
    element.addEventListener('click', (event) => {
      event.preventDefault();
      void action(element.dataset.action ?? '');
    }),
  );
  app.querySelectorAll<HTMLButtonElement>('[data-image]').forEach((element) =>
    element.addEventListener('click', () => {
      activeId = element.dataset.image ?? null;
      category = 'All';
      query = '';
      mode = results.has(activeId ?? '') ? 'result' : 'source';
      render();
      app.querySelector<HTMLButtonElement>(`[data-image="${activeId}"]`)?.focus();
    }),
  );
  app.querySelectorAll<HTMLInputElement>('[data-select]').forEach((element) =>
    element.addEventListener('change', () => {
      const id = element.dataset.select ?? '';
      if (element.checked) selected.add(id);
      else selected.delete(id);
      render();
      app.querySelector<HTMLInputElement>(`[data-select="${id}"]`)?.focus();
    }),
  );
  app.querySelector<HTMLInputElement>('#select-all')?.addEventListener('change', (event) => {
    const checked = (event.currentTarget as HTMLInputElement).checked;
    selected.clear();
    if (checked) images.forEach((image) => selected.add(image.id));
    render();
    app.querySelector<HTMLInputElement>('#select-all')?.focus();
  });
  app.querySelectorAll<HTMLButtonElement>('[data-mode]').forEach((element) =>
    element.addEventListener('click', () => {
      mode = element.dataset.mode === 'result' ? 'result' : 'source';
      category = 'All';
      query = '';
      render();
      app.querySelector<HTMLButtonElement>(`[data-mode="${mode}"]`)?.focus();
    }),
  );
  app.querySelector<HTMLInputElement>('#metadata-search')?.addEventListener('input', (event) => {
    query = (event.currentTarget as HTMLInputElement).value;
    refreshFields();
  });
  app.querySelector<HTMLSelectElement>('#category-filter')?.addEventListener('change', (event) => {
    category = (event.currentTarget as HTMLSelectElement).value;
    refreshFields();
  });
}
async function action(name: string): Promise<void> {
  try {
    feedback = '';
    if (name === 'dismiss-feedback') {
      feedback = '';
      render();
      return;
    }
    if (name === 'toggle-queue') {
      queueExpanded = !queueExpanded;
      render();
      return;
    }
    if (name === 'advanced') {
      advanced = !advanced;
      render();
      return;
    }
    if (name === 'policy') {
      showPolicy = !showPolicy;
      render();
      return;
    }
    if (name === 'dismiss') {
      message = '';
      render();
      return;
    }
    if (name === 'dismiss-results') {
      lastResults = [];
      render();
      return;
    }
    if (name === 'home') {
      return;
    }
    if (!isTauri()) {
      message = 'Image operations require the FauxMetadata Windows desktop app.';
      render();
      return;
    }
    if (name === 'cancel') {
      restoreOperationFocus ||=
        (document.activeElement as HTMLElement | null)?.dataset.action === 'cancel';
      cancelling = true;
      render();
      await invoke('cancel_operation');
      return;
    }
    if (busy || dialogOpen) return;
    if (name === 'add' || name === 'folder') {
      dialogOpen = true;
      const paths = await invoke<string[]>('choose_inputs', { folders: name === 'folder' });
      dialogOpen = false;
      if (paths.length) await importImages(paths);
    } else if (name === 'destination') {
      dialogOpen = true;
      const chosen = await invoke<Preferences | null>('choose_destination');
      dialogOpen = false;
      if (chosen) applyPreferences(chosen);
      optionsOpen = false;
      render();
    } else if (name === 'reset-destination') {
      applyPreferences(await invoke<Preferences>('reset_destination'));
      optionsOpen = false;
      render();
    } else if (name === 'open-last-output') {
      const id = batchOutcome(lastResults).firstVerifiedId;
      if (id) await invoke('open_cleaned_images', { id });
    } else if (name === 'open-output' || name === 'copy-output') {
      const image = activeImage();
      const output = image ? verifiedOutput(results.get(image.id)) : null;
      if (!output || !image) return;
      if (name === 'open-output') await invoke('open_cleaned_images', { id: image.id });
      else {
        await navigator.clipboard.writeText(displayPath(output));
        feedback = 'Cleaned image path copied.';
        render();
      }
    } else if (name === 'clear' || name === 'remove') {
      const ids = name === 'clear' ? images.map((image) => image.id) : activeId ? [activeId] : [];
      await invoke('remove_images', { ids });
      images = images.filter((image) => !ids.includes(image.id));
      ids.forEach((id) => {
        selected.delete(id);
        results.delete(id);
      });
      activeId = images[0]?.id ?? null;
      category = 'All';
      query = '';
      mode = 'source';
      lastResults = [];
      issues = [];
      render();
    } else if (name === 'export') await exportImages();
  } catch (error) {
    dialogOpen = false;
    message = String(error);
    render();
  }
}
async function importImages(paths: string[], fromDrop = false): Promise<void> {
  if (busy || dialogOpen || !ready) return;
  let autoIds: string[] = [];
  busy = 'import';
  optionsOpen = false;
  feedback = '';
  cancelling = false;
  message = '';
  issues = [];
  lastResults = [];
  progress = { completed: 0, total: paths.length, phase: 'importing', id: '' };
  render();
  try {
    const imported = await invoke<ImportResult>('import_images', { paths });
    const oldIds = new Set(images.map((image) => image.id));
    const added = imported.images.filter((image) => !oldIds.has(image.id));
    images.push(...added);
    added.forEach((image) => selected.add(image.id));
    issues = imported.issues;
    autoIds = autoCleanIds(
      preferences?.autoCleanOnDrop ?? false,
      fromDrop,
      cancelling,
      added.map((image) => image.id),
    );
    activeId = added[0]?.id ?? activeId ?? images[0]?.id ?? null;
    mode = 'source';
  } catch (error) {
    message = String(error);
  } finally {
    busy = null;
    cancelling = false;
    render();
    announce(
      `${images.length} image${images.length === 1 ? '' : 's'} in queue. ${issues.length} import notices.`,
    );
  }
  if (autoIds.length) await exportImages(autoIds);
}
async function exportImages(
  ids = advanced ? [...selected] : pendingIds(selected, results),
): Promise<void> {
  if (busy || !ids.length || !destination || !health || !ready) return;
  restoreOperationFocus =
    (document.activeElement as HTMLElement | null)?.dataset.action === 'export';
  busy = 'export';
  optionsOpen = false;
  feedback = '';
  cancelling = false;
  message = '';
  job = crypto.randomUUID();
  lastResults = [];
  ids.forEach((id) => results.delete(id));
  progress = { completed: 0, total: ids.length, phase: 'staging', id: '' };
  render();
  try {
    lastResults = await invoke<ExportResult[]>('export_images', { ids, job });
    lastResults.forEach((result) => results.set(result.id, result));
    activeId =
      lastResults.find((result) => !verifiedOutput(result))?.id ?? lastResults[0]?.id ?? activeId;
    mode = 'result';
    category = 'All';
    query = '';
  } catch (error) {
    message = String(error);
  } finally {
    busy = null;
    cancelling = false;
    render();
    announce(batchOutcome(lastResults).label);
    if (restoreOperationFocus) {
      focusActionButton(batchOutcome(lastResults).firstVerifiedId ? 'open-last-output' : 'export');
      restoreOperationFocus = false;
    }
  }
}
document.addEventListener('keydown', (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
    event.preventDefault();
    void action('export');
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'o') {
    event.preventDefault();
    void action('add');
  }
  if (event.key === 'Escape') {
    if (optionsOpen) {
      optionsOpen = false;
      render();
      app.querySelector<HTMLElement>('.destination-options summary')?.focus();
    } else if (showPolicy) {
      showPolicy = false;
      render();
    } else if (busy && !cancelling) void action('cancel');
  }
});
document.addEventListener('pointerdown', (event) => {
  if (
    optionsOpen &&
    event.target instanceof Element &&
    !event.target.closest('.destination-options')
  ) {
    optionsOpen = false;
    const details = app.querySelector<HTMLDetailsElement>('.destination-options');
    if (details) details.open = false;
  }
});

document.addEventListener('dragover', (event) => event.preventDefault());
document.addEventListener('drop', (event) => event.preventDefault());

render();
if (isTauri()) {
  void (async () => {
    try {
      // Install listeners before enabling the engine, so no progress events are lost.
      await listen<Progress>('export-progress', (event) => {
        if (event.payload.job !== job) return;
        const payload = event.payload;
        progress = {
          completed: payload.completed,
          total: payload.total,
          phase: payload.phase,
          id: payload.id,
        };
        if (payload.result) results.set(payload.id, payload.result);
        // Update the footer/queue without disturbing metadata selection and expanded values.
        updateProgress();
      });
      await listen<{ completed: number; total: number; name: string }>(
        'import-progress',
        (event) => {
          progress = {
            completed: event.payload.completed,
            total: event.payload.total,
            phase: 'importing',
            id: '',
          };
          updateProgress();
        },
      );
      const currentWindow = getCurrentWebviewWindow();
      await currentWindow.onResized(() => void syncWindowState());
      await currentWindow.onFocusChanged(({ payload: focused }) =>
        document.documentElement.classList.toggle('window-inactive', !focused),
      );
      void syncWindowState();
      await currentWindow.onDragDropEvent((event) => {
        const overlay = document.querySelector<HTMLElement>('#drop-overlay');
        if (!overlay) return;
        if (event.payload.type === 'enter' || event.payload.type === 'over')
          overlay.hidden = !!busy || dialogOpen || !ready;
        else overlay.hidden = true;
        if (event.payload.type === 'drop' && !busy && !dialogOpen && ready)
          void importImages(event.payload.paths, true);
      });
      applyPreferences(await invoke<Preferences>('get_preferences'));
      health = await invoke<Health>('health');
      ready = true;
      render();
    } catch (error) {
      engineFailed = true;
      message = `Local metadata engine unavailable: ${String(error)}`;
      render();
    }
  })();
}
function updateProgress(): void {
  const label = app.querySelector('.progress-text strong');
  if (label) label.textContent = cancelling ? 'Cancelling…' : phaseLabel(progress.phase);
  const count = app.querySelector('.progress-text span');
  if (count) count.textContent = `${progress.completed} of ${progress.total} images`;
  const bar = app.querySelector<HTMLProgressElement>('.batch-progress');
  if (bar) {
    bar.max = Math.max(progress.total, 1);
    bar.value = progress.completed;
    bar.setAttribute('aria-label', phaseLabel(progress.phase));
    bar.setAttribute(
      'aria-valuetext',
      `${progress.completed} of ${progress.total} images. ${phaseLabel(progress.phase)}`,
    );
  }
  const queue = app.querySelector('.queue');
  if (queue && busy === 'export' && !queue.contains(document.activeElement)) {
    queue.innerHTML = images.map(queueItem).join('');
    bindQueueProgress();
  }
  const status = app.querySelector('.quick-status strong');
  if (status && busy === 'export' && progress.id === activeId && progress.phase !== 'complete') {
    status.textContent = phaseLabel(progress.phase);
  }
  announce(`${phaseLabel(progress.phase)} ${progress.completed} of ${progress.total}.`);
}
function bindQueueProgress(): void {
  app.querySelectorAll<HTMLButtonElement>('[data-image]').forEach((element) =>
    element.addEventListener('click', () => {
      activeId = element.dataset.image ?? null;
      category = 'All';
      query = '';
      mode = results.has(activeId ?? '') ? 'result' : 'source';
      render();
    }),
  );
}
