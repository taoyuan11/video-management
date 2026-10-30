// Development-only asset export. Normal Cargo builds use the checked-in files.
// Usage: node scripts/generate-icons.cjs [path-to-sharp-module]
const fs = require('node:fs/promises');
const path = require('node:path');
const sharp = require(process.argv[2] || 'sharp');

const output = path.resolve(__dirname, '../assets/branding');
const sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

async function render(svg, size) {
  // Supersampling preserves rounded corners at title-bar and taskbar sizes.
  const large = await sharp(svg, { density: 288 }).png().toBuffer();
  return sharp(large).resize(size, size).png().toBuffer();
}

async function main() {
  const svg = await fs.readFile(path.join(output, 'frame-library.svg'));
  await fs.writeFile(path.join(output, 'frame-library.png'), await render(svg, 1024));
  const frames = await Promise.all(sizes.map(size => render(svg, size)));
  await fs.writeFile(path.join(output, 'frame-library-256.png'), frames.at(-1));
  // Windows 11 supports PNG-compressed, full-alpha ICO entries at every size.
  const header = Buffer.alloc(6 + sizes.length * 16);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(sizes.length, 4);
  let offset = header.length;
  sizes.forEach((size, index) => {
    const entry = 6 + index * 16;
    header[entry] = header[entry + 1] = size === 256 ? 0 : size;
    header.writeUInt16LE(1, entry + 4); // planes
    header.writeUInt16LE(32, entry + 6); // RGBA
    header.writeUInt32LE(frames[index].length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += frames[index].length;
  });
  await fs.writeFile(path.join(output, 'frame-library.ico'), Buffer.concat([header, ...frames]));
  console.log(`Exported logo PNG, window PNG and ${sizes.length}-size ICO to ${output}`);
}

main().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
