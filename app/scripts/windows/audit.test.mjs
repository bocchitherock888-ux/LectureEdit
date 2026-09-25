import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
function fakePE(machine=0x8664) {
  const b=Buffer.alloc(512);b.write('MZ');b.writeUInt32LE(128,0x3c);b.writeUInt32LE(0x4550,128);
  b.writeUInt16LE(machine,132);b.writeUInt16LE(240,148);b.writeUInt16LE(0x20b,152);
  b.writeUInt32LE(512,212);b.writeUInt32LE(16,260);return b;
}
function setup(t, multi=true) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'Suitang audit '));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));fs.mkdirSync(path.join(root,'qwen'));
  for(const name of ['lectureedit-loopback.exe','qwen/llama-server.exe',...(multi?['qwen/ggml-cpu-x64.dll','qwen/ggml-cpu-haswell.dll','qwen/ggml-vulkan.dll']:[])])
    fs.writeFileSync(path.join(root,name),fakePE());return root;
}
function run(root,...options) {
  return spawnSync(process.execPath,[path.join(import.meta.dirname,'audit-windows.mjs'),'--native-root',root,...options],{encoding:'utf8',timeout:15000});
}
test('CLI audits a complete synthetic multi-CPU payload and paths with spaces',t=>{const r=run(setup(t));assert.equal(r.status,0,r.stderr);const report=JSON.parse(r.stdout);assert.equal(report.records.length,5);assert.equal(report.inference,'not_run');assert.equal(report.dll_closure,'not_run')});
test('CLI checks baseline payload without dynamic CPU variants',t=>{const r=run(setup(t,false),'--profile','baseline');assert.equal(r.status,0,r.stderr)});
test('CLI fails for a missing baseline in multi mode',t=>{const root=setup(t);fs.unlinkSync(path.join(root,'qwen/ggml-cpu-x64.dll'));assert.notEqual(run(root).status,0)});
test('CLI fails when the multi build lacks the Vulkan backend',t=>{const root=setup(t);fs.unlinkSync(path.join(root,'qwen/ggml-vulkan.dll'));assert.match(run(root).stderr,/ggml-vulkan\.dll/)});
test('CLI rejects an ARM64 helper',t=>{const root=setup(t);fs.writeFileSync(path.join(root,'lectureedit-loopback.exe'),fakePE(0xaa64));assert.match(run(root).stderr,/expected x64/)});
test('CLI rejects model weights accidentally added to resources',t=>{const root=setup(t);fs.writeFileSync(path.join(root,'model.gguf'),'fixture');assert.match(run(root).stderr,/Unexpected runtime payload/)});
test('CLI checks the supplied app executable too',t=>{const root=setup(t);const app=path.join(root,'app.bin');fs.writeFileSync(app,fakePE(0xaa64));const outside=path.join(os.tmpdir(),`Suitang-test-app-${process.pid}-${Date.now()}.exe`);fs.renameSync(app,outside);t.after(()=>fs.rmSync(outside,{force:true}));assert.match(run(root,'--app-exe',outside).stderr,/expected x64/)});
