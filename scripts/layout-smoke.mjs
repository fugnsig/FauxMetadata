// Real production frontend in headless Edge; native IPC is explicitly mocked.
// No dependency install, production test hook or changes to the app's CSP.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { resolve, join, extname, sep } from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';

const { version } = JSON.parse(readFileSync('package.json', 'utf8'));
const edge =
  process.env.EDGE_PATH ?? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
if (!existsSync(edge)) throw new Error('Set EDGE_PATH to the installed Microsoft Edge executable.');
const dist = resolve('dist');
const preview = readFileSync('artifacts/qa-input/ComfyUI_landscape_00001.png').toString('base64');
const profile = mkdtempSync(join(tmpdir(), 'fauxmetadata-layout-'));
const delay = (ms) => new Promise((done) => setTimeout(done, ms));
const server = createServer((request, response) => {
  const path = resolve(
    dist,
    '.' +
      decodeURIComponent(
        new URL(request.url, 'http://localhost').pathname === '/'
          ? '/index.html'
          : new URL(request.url, 'http://localhost').pathname,
      ),
  );
  if (!path.startsWith(dist + sep) || !existsSync(path)) {
    response.writeHead(404).end();
    return;
  }
  response.setHeader(
    'Content-Type',
    { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' }[extname(path)] ??
      'application/octet-stream',
  );
  response.end(readFileSync(path));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const browser = spawn(
  edge,
  [
    '--headless=new',
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--remote-debugging-port=0',
    `--user-data-dir=${profile}`,
    'about:blank',
  ],
  { windowsHide: true, stdio: 'ignore' },
);
let socket;
const pending = new Map();
let nextId = 0;
function send(method, params = {}) {
  const id = ++nextId;
  return new Promise((done, fail) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      fail(new Error(`${method} timed out`));
    }, 15000);
    pending.set(id, {
      done: (value) => {
        clearTimeout(timer);
        done(value);
      },
      fail: (error) => {
        clearTimeout(timer);
        fail(error);
      },
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
}
async function until(expression) {
  for (let attempt = 0; attempt < 120; attempt++) {
    if (await evaluate(expression)) return;
    await delay(50);
  }
  throw new Error(
    `Frontend did not settle: ${expression}\n${await evaluate(`JSON.stringify({text:document.body.innerText,focus:document.activeElement?.outerHTML,exports:window.__qa?.exports})`)}`,
  );
}
const sizes = [
  [480, 480],
  [760, 580],
  [960, 640],
  [980, 700],
  [1240, 820],
  [1920, 1080],
  [3440, 1440],
  [400, 600],
];
const observations = [];
async function layout(label) {
  for (const [width, height] of sizes) {
    await send('Emulation.setDeviceMetricsOverride', {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await evaluate(
      'new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))',
    );
    const metrics = await evaluate(`(() => {
      const box = selector => { const b=document.querySelector(selector)?.getBoundingClientRect(); return b && {left:b.left,top:b.top,right:b.right,bottom:b.bottom,width:b.width,height:b.height}; };
      return {width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,export:box('.export-bar'),clean:box('[data-action="export"], [data-action="cancel"]'),brand:box('.brand'),controls:box('.window-controls'),switch:box('.view-switch'),preview:box('.quick-image img'),result:box('.quick-result'),focus:document.activeElement?.id,font:parseFloat(getComputedStyle(document.documentElement).fontSize),queue:document.querySelector('.count')?.textContent};
    })()`);
    observations.push({ label, ...metrics });
    assert.ok(
      metrics.scrollWidth <= width + 1,
      `${label} ${width}: horizontal page overflow ${metrics.scrollWidth}`,
    );
    assert.ok(
      metrics.export.top >= 0 && metrics.export.bottom <= height + 1,
      `${label} ${width}x${height}: export controls outside viewport ${JSON.stringify(metrics.export)}`,
    );
    assert.ok(
      metrics.clean.height > 0 && metrics.clean.left >= 0 && metrics.clean.right <= width + 1,
      `${label} ${width}: primary action clipped`,
    );
    assert.ok(
      metrics.brand.right <= metrics.switch.left + 1,
      `${label} ${width}: header branding overlaps view switch`,
    );
    assert.ok(
      metrics.controls &&
        metrics.controls.left >= metrics.switch.right &&
        metrics.controls.right <= width + 1 &&
        metrics.controls.top <= 1,
      `${label} ${width}: window controls missing, clipped or overlapping`,
    );
    if (metrics.preview && metrics.result)
      assert.ok(
        metrics.preview.bottom <= metrics.result.top + 1,
        `${label} ${width}: preview overlaps result`,
      );
    assert.ok(
      metrics.font >= 14 && metrics.font <= 18,
      `${label}: typography outside bounded scale`,
    );
    if (label === 'verified')
      assert.equal(
        await evaluate(
          `getComputedStyle(document.querySelector('[data-action="open-last-output"] span')).display !== 'none'`,
        ),
        true,
        'Verified folder action label must remain visible',
      );
    if (
      ['empty', 'queued', 'verified', 'processing', 'advanced-report', 'webp-verified'].includes(
        label,
      ) &&
      [480, 960, 980, 1240].includes(width)
    ) {
      mkdirSync('artifacts/layout', { recursive: true });
      // Captures are documentation images; hide test keyboard focus, then restore it unchanged.
      await evaluate(
        `(()=>{window.__qaFocus=document.activeElement;const s=document.createElement('style');s.id='qa-capture';s.textContent='*:focus-visible{outline:none!important}';document.head.append(s);})()`,
      );
      const screenshot = await send('Page.captureScreenshot', { format: 'png' });
      await evaluate(`document.querySelector('#qa-capture').remove()`);
      assert.equal(
        await evaluate('document.activeElement === window.__qaFocus'),
        true,
        `${label} ${width}: capture changed focus`,
      );
      writeFileSync(
        `artifacts/layout/${label}-${width}.png`,
        Buffer.from(screenshot.data, 'base64'),
      );
    }
  }
}
try {
  const portFile = join(profile, 'DevToolsActivePort');
  for (let attempt = 0; attempt < 120 && !existsSync(portFile); attempt++) await delay(50);
  if (!existsSync(portFile)) throw new Error('Headless Edge did not start.');
  const port = readFileSync(portFile, 'utf8').split('\n')[0];
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(targets.find((target) => target.type === 'page').webSocketDebuggerUrl);
  await new Promise((done, fail) => {
    socket.addEventListener('open', done, { once: true });
    socket.addEventListener('error', fail, { once: true });
  });
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    const callback = pending.get(message.id);
    if (callback) {
      pending.delete(message.id);
      if (message.error) callback.fail(new Error(JSON.stringify(message.error)));
      else callback.done(message.result);
    }
  });
  await send('Page.enable');
  await send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await send('Page.addScriptToEvaluateOnNewDocument', {
    source: `
    window.isTauri=true;
    const bs=String.fromCharCode(92);
    // Illustrative paths only; the real app resolves the native Windows known folder.
    const pictures=['C:','Users','Example','Pictures'].join(bs);
    const folder=pictures+bs+'Cleaned Images';
    const fields=[{key:'PNG:prompt',value:'private prompt',category:'Generation',retained:false},{key:'PNG:workflow',value:'{"nodes":[],"links":[]}',category:'Generation',retained:false}];
    window.__qa={callbacks:new Map(),events:new Map(),next:0,cancel:false,fail:false,hold:false,mixed:false,batchSize:1,longNames:false,imports:0,exports:0,opened:null,window:[],preferences:{destination:folder,isDefault:true,autoCleanOnDrop:false,error:null}};
    window.__qa.imageName=i=>__qa.longNames?'ComfyUI 花 '+ 'long filename '.repeat(5)+i+'.png':'ComfyUI_'+String(i+42).padStart(5,'0')+(__qa.webp?'.webp':'.png');
    window.__qa.images=()=>Array.from({length:__qa.batchSize},(_,i)=>({id:String(i+1),name:__qa.imageName(i),path:pictures+bs+__qa.imageName(i),format:__qa.webp?'WebP':'PNG',bytes:6290,width:960,height:640,preview:'data:image/png;base64,${preview}',fields,chunks:__qa.webp?['VP8X','VP8L','EXIF','XMP ']:['IHDR','IDAT','tEXt','IEND']}));
    window.__qa.emit=(event,payload)=>{const id=__qa.events.get(event);if(id)__qa.callbacks.get(id)({event,payload});};
    window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},transformCallback:callback=>{const id=++__qa.next;__qa.callbacks.set(id,callback);return id;},invoke:async(cmd,args)=>{
      if(cmd==='plugin:event|listen'){__qa.events.set(args.event,args.handler);return ++__qa.next;}
      if(cmd==='get_preferences')return __qa.preferences;
      if(cmd==='health'){
        if(new URL(location.href).searchParams.has('engine-error'))throw new Error('Synthetic missing ExifTool');
        await new Promise(done=>{__qa.healthRelease=done;});
        return {version:${JSON.stringify(version)},exiftool:'13.60',offline:true};
      }
      if(cmd==='choose_inputs')return [pictures];
      if(cmd==='choose_destination'){__qa.preferences={...__qa.preferences,destination:[bs+bs+'server','Redirected Pictures 花','a long folder name '.repeat(9),'Cleaned Images'].join(bs),isDefault:false};return __qa.preferences;}
      if(cmd==='reset_destination'){__qa.preferences={...__qa.preferences,destination:folder,isDefault:true};return __qa.preferences;}
      if(cmd==='set_auto_clean'){__qa.preferences={...__qa.preferences,autoCleanOnDrop:args.enabled};return __qa.preferences;}
      if(cmd==='import_images'){__qa.imports++;return {images:__qa.images(),issues:__qa.longNames?[{path:'malformed.png',message:'Unsupported PNG; no image imported.'}]:[]};}
      if(cmd==='remove_images')return;
      if(cmd.startsWith('plugin:window|')){__qa.window.push(cmd);return cmd==='plugin:window|is_maximized'?false:undefined;}
      if(cmd==='open_cleaned_images'){__qa.opened=args.id;return;}
      if(cmd==='cancel_operation'){__qa.cancel=true;__qa.release?.();return;}
      if(cmd==='export_images'){
        __qa.exports++; __qa.cancel=false;
        __qa.emit('export-progress',{job:args.job,id:args.ids[0],phase:'stripping',completed:0,total:args.ids.length,result:null});
        await new Promise(done=>{__qa.release=done;if(!__qa.hold)setTimeout(done,200);});
        return args.ids.map((id,i)=>{const status=__qa.cancel?'cancelled':__qa.fail?'error':__qa.mixed&&i%3===1?'error':__qa.mixed&&i%3===2?'cancelled':'verified';const name=__qa.imageName(Number(id)-1);const dot=name.lastIndexOf('.');return {id,status,output:status==='verified'?__qa.preferences.destination+bs+name.slice(0,dot)+'_clean'+name.slice(dot):null,error:status==='verified'?null:'Synthetic filesystem/cancellation failure',removed:fields,retained:[],after:[],removedChunks:__qa.webp?['EXIF','XMP ']:['tEXt'],bytes:5837,encodedVerified:status==='verified',pixelsVerified:status==='verified'};});
      }
      throw new Error('Unexpected mock command: '+cmd);
    }};

  `,
  });
  await send('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` });
  await until(`!!window.__qa?.healthRelease`);
  await layout('loading');
  assert.equal(await evaluate(`document.querySelector('[data-action="add"]').disabled`), true);
  await evaluate(`__qa.healthRelease()`);
  await until(`document.querySelector('.footer')?.textContent.includes('13.60')`);
  await layout('empty');
  await evaluate(
    `['minimise','maximise','close'].forEach(n=>document.querySelector('#window-'+n).click())`,
  );
  assert.deepEqual(await evaluate(`__qa.window.filter((c) => !c.endsWith('is_maximized'))`), [
    'plugin:window|minimize',
    'plugin:window|toggle_maximize',
    'plugin:window|close',
  ]);
  const theme = await evaluate(`(() => {
    const style=getComputedStyle(document.documentElement);
    return Object.fromEntries(['--canvas','--surface','--surface-raised','--text','--text-secondary','--text-muted','--accent','--accent-hover','--accent-pressed','--accent-ink','--warning','--error'].map(k=>[k,style.getPropertyValue(k).trim()]));
  })()`);
  assert.equal(theme['--canvas'], '#0b1218');
  assert.equal(theme['--accent'], '#58f777');
  const luminance = (hex) => {
    const rgb = hex
      .slice(1)
      .match(/../g)
      .map((v) => parseInt(v, 16) / 255)
      .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
    return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
  };
  const contrast = (a, b) =>
    (Math.max(luminance(a), luminance(b)) + 0.05) / (Math.min(luminance(a), luminance(b)) + 0.05);
  for (const foreground of ['--text', '--text-secondary', '--text-muted', '--warning', '--error'])
    for (const background of ['--canvas', '--surface', '--surface-raised'])
      assert.ok(
        contrast(theme[foreground], theme[background]) >= 4.5,
        `${foreground} fails small-text contrast on ${background}`,
      );
  for (const background of ['--accent', '--accent-hover', '--accent-pressed'])
    assert.ok(contrast(theme['--accent-ink'], theme[background]) >= 4.5, 'Primary action contrast');
  assert.ok(
    await evaluate(`document.querySelector('.supported-formats').textContent.includes('WebP')`),
  );
  await evaluate(`document.querySelector('.brand').focus()`);
  await send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: 'Tab',
    code: 'Tab',
    windowsVirtualKeyCode: 9,
  });
  await send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: 'Tab',
    code: 'Tab',
    windowsVirtualKeyCode: 9,
  });
  assert.equal(await evaluate('document.activeElement.dataset.action'), 'advanced');
  assert.equal(await evaluate(`getComputedStyle(document.activeElement).outlineWidth`), '2px');
  // Real DOM key event path, followed by CDP key activation of the focused native control.
  await evaluate(
    `document.dispatchEvent(new KeyboardEvent('keydown',{key:'o',ctrlKey:true,bubbles:true}))`,
  );
  await until(`document.querySelector('.count')?.textContent==='1'`);
  await layout('queued');
  await evaluate(`document.querySelector('[data-action="toggle-queue"]').click()`);
  await layout('queue-expanded');
  assert.equal(
    await evaluate(
      `getComputedStyle(document.querySelector('.queue-panel [data-action="clear"]')).display !== 'none' && document.querySelector('.queue-panel [data-action="clear"]').getBoundingClientRect().height > 0`,
    ),
    true,
  );
  assert.equal(
    await evaluate(
      `document.querySelector('.toolbar-actions [data-action="folder"]').getBoundingClientRect().width > 0`,
    ),
    true,
  );
  assert.equal(
    await evaluate(
      `document.querySelector('[data-action="toggle-queue"]').getAttribute('aria-expanded')`,
    ),
    'true',
  );
  await evaluate(
    `document.querySelector('[data-action="toggle-queue"]').click();document.querySelector('.destination-options').open=true`,
  );
  await evaluate(`document.querySelector('#auto-clean').focus()`);
  await layout('focus-preserved');
  assert.equal(await evaluate('document.activeElement.id'), 'auto-clean');
  await evaluate(`document.querySelector('[data-action="export"]').focus()`);
  await send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: 'Enter',
    code: 'Enter',
    windowsVirtualKeyCode: 13,
    text: '\r',
    unmodifiedText: '\r',
  });
  await send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: 'Enter',
    code: 'Enter',
    windowsVirtualKeyCode: 13,
  });
  await until(`!!document.querySelector('[data-action="copy-output"]')`);
  assert.equal(await evaluate(`document.activeElement.dataset.action`), 'open-last-output');
  assert.equal(await evaluate(`document.querySelector('.destination-options').open`), false);
  await layout('verified');
  await evaluate(`document.querySelector('[data-action="open-last-output"]').click()`);
  assert.equal(await evaluate('__qa.opened'), '1');
  // Capture the example report with the familiar default, before path stress checks.
  await evaluate(
    `document.querySelector('[data-action="advanced"]').click();document.querySelector('.diff').open=true;`,
  );
  await layout('advanced-report');
  assert.equal(await evaluate(`document.querySelector('.diff').open`), true);
  await evaluate(`document.querySelector('[data-action="advanced"]').click()`);
  await evaluate(`document.querySelector('[data-action="destination"]').click()`);
  await until(`!document.querySelector('.default-label')`);
  assert.equal(
    await evaluate(`document.querySelector('.batch-location').textContent`),
    ['C:', 'Users', 'Example', 'Pictures', 'Cleaned Images'].join(String.fromCharCode(92)),
  );
  await layout('long-destination');
  await evaluate(
    `document.querySelector('[data-action="advanced"]').click();document.querySelector('.diff').open=true;`,
  );
  await evaluate(
    `document.querySelector('[data-mode="source"]').click();document.querySelector('#metadata-search').value='prompt';document.querySelector('#metadata-search').dispatchEvent(new Event('input'));document.querySelector('.metadata-field').open=true;`,
  );
  await layout('advanced-inspection');
  assert.equal(await evaluate(`document.querySelector('#metadata-search').value`), 'prompt');
  // A same-view rerender must preserve expanded untrusted metadata and focus.
  await evaluate(
    `document.querySelector('#metadata-search').focus();document.querySelector('[data-action="policy"]').click()`,
  );
  assert.equal(await evaluate(`document.querySelector('.metadata-field').open`), true);
  assert.equal(await evaluate(`document.activeElement.id`), 'metadata-search');
  await evaluate(
    `document.querySelector('[data-action="policy"]').click();__qa.fail=true;document.querySelector('[data-action="export"]').click()`,
  );
  await until(`document.querySelector('.report.error')?.textContent.includes('not published')`);
  await layout('advanced-error');
  await evaluate(`document.querySelector('[data-action="advanced"]').click()`);
  await layout('quick-error');
  await evaluate(
    `__qa.fail=false;__qa.hold=true;document.querySelector('[data-action="export"]').focus();document.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',ctrlKey:true,bubbles:true}))`,
  );
  await until(`!!document.querySelector('[data-action="cancel"]')`);
  assert.equal(await evaluate(`document.activeElement.dataset.action`), 'cancel');
  assert.equal(
    await evaluate(`document.querySelector('.progress-text strong').textContent`),
    'Removing metadata…',
  );
  assert.equal(await evaluate(`!!document.querySelector('.batch-summary.verified')`), false);
  await layout('processing');
  await send('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-reduced-motion', value: 'reduce' }],
  });
  assert.equal(
    await evaluate(`getComputedStyle(document.querySelector('.spinner')).animationName`),
    'none',
  );
  await send('Emulation.setEmulatedMedia', {
    features: [{ name: 'forced-colors', value: 'active' }],
  });
  assert.equal(await evaluate(`matchMedia('(forced-colors: active)').matches`), true);
  assert.equal(
    await evaluate(
      `getComputedStyle(document.querySelector('[data-action="cancel"]')).borderTopWidth`,
    ),
    '1px',
  );
  await send('Emulation.setEmulatedMedia', { features: [] });
  await evaluate(
    `document.querySelector('[data-action="cancel"]').focus();document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`,
  );
  await until(`document.querySelector('.quick-status')?.textContent.includes('Cancelled')`);
  assert.equal(await evaluate(`document.activeElement.dataset.action`), 'export');
  await layout('cancelled');
  // A new batch forgets queue IDs; it never calls a source-file deletion command.
  await evaluate(
    `document.querySelector('.batch-actions [data-action="clear"]').focus();document.querySelector('.batch-actions [data-action="clear"]').click()`,
  );
  await until(`!!document.querySelector('.empty-state')`);
  assert.equal(await evaluate(`document.activeElement.dataset.action`), 'add');
  await evaluate(
    `__qa.batchSize=32;__qa.longNames=true;document.querySelector('[data-action="add"]').click()`,
  );
  await until(`document.querySelector('.count')?.textContent==='32'`);
  await layout('large-queue');
  await evaluate(
    `__qa.hold=false;__qa.mixed=true;document.querySelector('[data-action="export"]').click()`,
  );
  await until(`!!document.querySelector('.batch-summary.warning')`);
  await layout('mixed-results');
  assert.ok(
    (await evaluate(`document.querySelector('.batch-copy strong').textContent`)).includes('failed'),
  );
  await evaluate(`document.querySelector('[data-action="reset-destination"]').click()`);
  await until(`!!document.querySelector('.default-label')`);
  // Drop feedback and opt-in automatic processing use the actual frontend event listeners.
  await evaluate(`document.querySelector('[data-action="clear"]').click()`);
  await until(`!!document.querySelector('.empty-state')`);
  await evaluate(
    `__qa.batchSize=1;__qa.longNames=false;__qa.mixed=false;__qa.emit('tauri://drag-enter',{paths:['C:/ComfyUI'],position:{x:1,y:1}})`,
  );
  assert.equal(await evaluate(`document.querySelector('#drop-overlay').hidden`), false);
  await evaluate(`__qa.emit('tauri://drag-leave',{})`);
  assert.equal(await evaluate(`document.querySelector('#drop-overlay').hidden`), true);
  const beforeDrop = await evaluate('__qa.exports');
  await evaluate(`__qa.emit('tauri://drag-drop',{paths:['C:/ComfyUI'],position:{x:1,y:1}})`);
  await until(`document.querySelector('.count')?.textContent==='1'`);
  assert.equal(await evaluate('__qa.exports'), beforeDrop);
  await evaluate(`document.querySelector('.destination-options').open=true`);
  await evaluate(`document.querySelector('#auto-clean').click()`);
  await until(
    `!document.querySelector('#auto-clean').disabled && document.querySelector('#auto-clean').checked`,
  );
  await evaluate(`document.querySelector('[data-action="clear"]').click()`);
  await until(`!!document.querySelector('.empty-state')`);
  await evaluate(`__qa.emit('tauri://drag-drop',{paths:['C:/ComfyUI'],position:{x:1,y:1}})`);
  await until(`!!document.querySelector('.batch-summary.verified')`);
  assert.equal(await evaluate('__qa.exports'), beforeDrop + 1);
  await evaluate(`document.querySelector('.batch-actions [data-action="clear"]').click()`);
  await until(`!!document.querySelector('.empty-state')`);
  await evaluate(`__qa.webp=true;document.querySelector('[data-action="add"]').click()`);
  await until(`document.querySelector('.preview-heading')?.textContent.includes('.webp')`);
  assert.ok(
    await evaluate(`document.querySelector('.preview-heading').textContent.includes('WebP')`),
  );
  await layout('webp-queued');
  await evaluate(`document.querySelector('[data-action="export"]').click()`);
  await until(`document.querySelector('.quick-result')?.textContent.includes('_clean.webp')`);
  await layout('webp-verified');
  await send('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/?engine-error=1` });
  await until(`document.querySelector('.notice.error')?.textContent.includes('unavailable')`);
  await layout('engine-error');
  assert.equal(await evaluate(`document.querySelector('[data-action="add"]').disabled`), true);
  mkdirSync('artifacts/layout', { recursive: true });
  writeFileSync(
    'artifacts/layout/report.json',
    JSON.stringify(
      {
        engine: 'Headless Edge, real production frontend; mocked native IPC',
        checks: observations.length,
        observations,
      },
      null,
      2,
    ) + '\n',
  );
  console.log(
    `Passed ${observations.length} viewport/state combinations; focus, reports and search survive resizing.`,
  );
} finally {
  if (socket?.readyState === WebSocket.OPEN) {
    try {
      await send('Browser.close');
    } catch {
      /* Browser may close before replying. */
    }
    socket.close();
  }
  if (browser.exitCode === null) {
    await Promise.race([new Promise((done) => browser.once('exit', done)), delay(3000)]);
  }
  if (browser.exitCode === null) browser.kill();
  await delay(500);
  server.close();
  if (resolve(profile).startsWith(join(tmpdir(), 'fauxmetadata-layout-'))) {
    try {
      rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    } catch {
      console.warn(`Browser profile retained: ${profile}`);
    }
  }
}
