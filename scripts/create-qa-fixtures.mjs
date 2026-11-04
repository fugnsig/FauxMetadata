// Local synthetic images only. This is a developer QA utility, never shipped in the app.
import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const directory = resolve('artifacts/qa-input');
mkdirSync(directory, { recursive: true });
mkdirSync(resolve('artifacts/qa-output'), { recursive: true });
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}
const width = 960,
  height = 640;
const raster = Buffer.alloc((width * 3 + 1) * height);
for (let y = 0; y < height; y++)
  for (let x = 0; x < width; x++) {
    const at = y * (width * 3 + 1) + 1 + x * 3;
    const wave = Math.sin(x / 190) * 50 + 360;
    const front = Math.sin(x / 165 + 1.7) * 70 + 475;
    const sun = (x - 690) ** 2 + (y - 170) ** 2 < 55 ** 2;
    const colour =
      y > front
        ? [30, 64, 63]
        : y > wave
          ? [67, 109, 101]
          : sun
            ? [239, 216, 163]
            : [118 + y / 10, 155 + y / 14, 157 + y / 25];
    for (let channel = 0; channel < 3; channel++) raster[at + channel] = colour[channel];
  }
const header = Buffer.alloc(13);
header.writeUInt32BE(width);
header.writeUInt32BE(height, 4);
header[8] = 8;
header[9] = 2;
const chunks = [
  chunk('IHDR', header),
  chunk(
    'tEXt',
    Buffer.from(
      'workflow\0' +
        JSON.stringify({
          nodes: [
            { id: 1, type: 'KSampler' },
            { id: 2, type: 'CLIPTextEncode' },
          ],
          links: [[1, 2, 0, 1, 0, 'CONDITIONING']],
        }),
    ),
  ),
  chunk(
    'tEXt',
    Buffer.from(
      'prompt\0' +
        JSON.stringify({
          1: {
            class_type: 'KSampler',
            inputs: {
              seed: 7429183,
              steps: 28,
              cfg: 7,
              sampler_name: 'euler',
              model: 'private-model.safetensors',
            },
          },
          2: {
            class_type: 'CLIPTextEncode',
            inputs: { text: 'quiet mountains, warm sky, editorial landscape' },
          },
        }),
    ),
  ),
  chunk('tEXt', Buffer.from('Software\0ComfyUI')),
  chunk('tEXt', Buffer.from('Author\0Synthetic QA fixture')),
  chunk('IDAT', deflateSync(raster)),
  chunk('IEND', Buffer.alloc(0)),
];
const png = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), ...chunks]);
writeFileSync(resolve(directory, 'ComfyUI_landscape_00001.png'), png);
writeFileSync(resolve(directory, 'Unicode 花 [draft].png'), png);
writeFileSync(resolve(directory, 'malformed.png'), Buffer.from('not a PNG image'));
console.log(`QA fixtures created in ${directory}`);
