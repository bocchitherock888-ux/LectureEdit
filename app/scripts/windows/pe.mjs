/** Small bounded PE reader. It reads headers/import names; it never executes a binary. */
export function readPE(buffer, name = '<buffer>') {
  function need(offset, size) {
    if (!Number.isSafeInteger(offset) || offset < 0 || offset + size > buffer.length)
      throw new Error(`${name}: truncated or invalid PE offset ${offset}`);
  }
  need(0, 64);
  if (buffer.toString('ascii', 0, 2) !== 'MZ') throw new Error(`${name}: missing MZ signature`);
  const pe = buffer.readUInt32LE(0x3c);
  need(pe, 24);
  if (buffer.readUInt32LE(pe) !== 0x4550) throw new Error(`${name}: missing PE signature`);
  const machine = buffer.readUInt16LE(pe + 4);
  const sectionCount = buffer.readUInt16LE(pe + 6);
  const optionalSize = buffer.readUInt16LE(pe + 20);
  const optional = pe + 24;
  need(optional, optionalSize);
  if (optionalSize < 112 || buffer.readUInt16LE(optional) !== 0x20b)
    throw new Error(`${name}: expected PE32+ executable`);
  const headersSize = buffer.readUInt32LE(optional + 60);
  const directories = buffer.readUInt32LE(optional + 108);
  const sections = [];
  for (let i = 0; i < sectionCount; i++) {
    const at = optional + optionalSize + i * 40; need(at, 40);
    sections.push({ virtual: buffer.readUInt32LE(at + 12), size: buffer.readUInt32LE(at + 16),
      raw: buffer.readUInt32LE(at + 20) });
  }
  function offsetOf(rva, length = 1) {
    if (rva < headersSize) { need(rva, length); return rva; }
    for (const section of sections) {
      const delta = rva - section.virtual;
      if (delta >= 0 && delta + length <= section.size) {
        const at = section.raw + delta; need(at, length); return at;
      }
    }
    throw new Error(`${name}: RVA ${rva} is outside the file-backed sections`);
  }
  function cstring(rva) {
    const at = offsetOf(rva); let end = at;
    while (end < buffer.length && end - at < 512 && buffer[end] !== 0) end++;
    if (end === buffer.length || end - at === 512) throw new Error(`${name}: unterminated import name`);
    const value = buffer.toString('ascii', at, end);
    if (!/^[A-Za-z0-9_.+-]+\.dll$/i.test(value)) throw new Error(`${name}: invalid import name ${value}`);
    return value;
  }
  const imports = new Set();
  for (const [index, stride, nameField] of [[1, 20, 12], [13, 32, 4]]) {
    if (directories <= index) continue;
    const directory = optional + 112 + index * 8;
    if (directory + 8 > optional + optionalSize) throw new Error(`${name}: invalid directory count`);
    const rva = buffer.readUInt32LE(directory), size = buffer.readUInt32LE(directory + 4);
    if (!rva || !size) continue;
    for (let n = 0; n + stride <= size; n += stride) {
      const at = offsetOf(rva + n, stride);
      if (buffer.subarray(at, at + stride).every(byte => byte === 0)) break;
      if (index === 13 && !(buffer.readUInt32LE(at) & 1))
        throw new Error(`${name}: legacy VA-based delay import unsupported; audit manually`);
      imports.add(cstring(buffer.readUInt32LE(at + nameField)));
    }
  }
  return { machine, architecture: machine === 0x8664 ? 'x64' : machine === 0xaa64 ? 'arm64' : `0x${machine.toString(16)}`,
    imports: [...imports].sort() };
}
export function assertX64(buffer, name) {
  const info = readPE(buffer, name);
  if (info.machine !== 0x8664) throw new Error(`${name}: expected x64/AMD64 (0x8664), found ${info.architecture}`);
  return info;
}
export function isApiSet(name) { return /^(api|ext)-ms-[a-z0-9_.-]+\.dll$/i.test(name); }
export function requiresSeparateMsvcRuntime(name) { return /^(?:(?:vcruntime|msvcp|concrt|vcomp)\d.*|libomp(?:\d.*)?)\.dll$/i.test(name); }
