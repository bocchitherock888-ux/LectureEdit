import test from 'node:test';
import assert from 'node:assert/strict';
import { readPE, assertX64, isApiSet, requiresSeparateMsvcRuntime } from './pe.mjs';
function fixture(machine=0x8664) {
  const b=Buffer.alloc(1024); b.write('MZ'); b.writeUInt32LE(128,0x3c); b.writeUInt32LE(0x4550,128);
  b.writeUInt16LE(machine,132); b.writeUInt16LE(0,134); b.writeUInt16LE(240,148);
  b.writeUInt16LE(0x20b,152); b.writeUInt32LE(1024,212); b.writeUInt32LE(16,260); return b;
}
test('recognises an x64 PE32+ executable',()=>assert.equal(assertX64(fixture(),'app').architecture,'x64'));
test('rejects ARM64 even though it is PE32+',()=>assert.throws(()=>assertX64(fixture(0xaa64),'app'),/expected x64/));
test('rejects Mach-O renamed as exe',()=>assert.throws(()=>assertX64(Buffer.alloc(128),'app'),/MZ/));
test('rejects truncated DOS header',()=>assert.throws(()=>readPE(Buffer.alloc(16)),/truncated/));
test('bounds checks PE header pointer',()=>{const b=fixture();b.writeUInt32LE(0xfffffffe,0x3c);assert.throws(()=>readPE(b),/offset/)});
test('rejects incorrect PE signature',()=>{const b=fixture();b.writeUInt32LE(123,128);assert.throws(()=>readPE(b),/signature/)});
test('rejects PE32 instead of PE32+',()=>{const b=fixture();b.writeUInt16LE(0x10b,152);assert.throws(()=>readPE(b),/PE32\+/)});
test('reads normal import names',()=>{const b=fixture();b.writeUInt32LE(512,272);b.writeUInt32LE(40,276);b.writeUInt32LE(700,524);b.write('KERNEL32.dll\0',700);assert.deepEqual(readPE(b).imports,['KERNEL32.dll'])});
test('reads delay imports',()=>{const b=fixture();b.writeUInt32LE(512,368);b.writeUInt32LE(64,372);b.writeUInt32LE(1,512);b.writeUInt32LE(700,516);b.write('USER32.dll\0',700);assert.deepEqual(readPE(b).imports,['USER32.dll'])});
test('rejects path traversal in import names',()=>{const b=fixture();b.writeUInt32LE(512,272);b.writeUInt32LE(40,276);b.writeUInt32LE(700,524);b.write('../evil.dll\0',700);assert.throws(()=>readPE(b),/invalid import/)});
test('rejects out of bounds import RVA',()=>{const b=fixture();b.writeUInt32LE(4000,272);b.writeUInt32LE(40,276);assert.throws(()=>readPE(b),/RVA/)});
test('separates Windows API contracts from local DLLs',()=>{assert.ok(isApiSet('api-ms-win-core-file-l1-1-0.dll'));assert.ok(!isApiSet('ggml-cpu.dll'))});
test('detects unsupported extra C++ or OpenMP runtimes',()=>{for(const n of ['VCRUNTIME140.dll','MSVCP140.dll','vcomp140.dll','libomp.dll','libomp140.dll'])assert.ok(requiresSeparateMsvcRuntime(n));assert.ok(!requiresSeparateMsvcRuntime('msvcrt.dll'))});
