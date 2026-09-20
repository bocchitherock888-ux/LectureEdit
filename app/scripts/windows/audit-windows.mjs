import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { assertX64, isApiSet, requiresSeparateMsvcRuntime } from './pe.mjs';

const argumentsList = process.argv.slice(2);
function option(name, fallback) {
  const index = argumentsList.indexOf(name);
  if (index < 0) return fallback;
  if (!argumentsList[index + 1] || argumentsList[index + 1].startsWith('--')) throw new Error(`Missing value for ${name}`);
  return argumentsList[index + 1];
}
const flags = new Set(['--native-root','--profile','--app-exe','--out','--dll-check','--smoke']);
for (const arg of argumentsList) if (arg.startsWith('--') && !flags.has(arg)) throw new Error(`Unknown option ${arg}`);
const root = path.resolve(option('--native-root', path.join(import.meta.dirname, '../../src-tauri/resources/native')));
const profile = option('--profile', 'multi');
if (!['multi','baseline'].includes(profile)) throw new Error('Expected --profile multi or baseline');
const dllCheck = argumentsList.includes('--dll-check');
const smoke = argumentsList.includes('--smoke');
if ((dllCheck || smoke) && process.platform !== 'win32') throw new Error('--dll-check/--smoke requires real Windows');
const required = ['lectureedit-loopback.exe','qwen/llama-server.exe'];
if (profile === 'multi') required.push('qwen/ggml-cpu-x64.dll');
for (const relative of required) if (!fs.statSync(path.join(root, relative), {throwIfNoEntry:false})?.isFile()) throw new Error(`Missing packaged native file ${relative}`);
if (profile === 'multi' && !fs.readdirSync(path.join(root,'qwen')).some(name => /^ggml-cpu-(haswell|sandybridge|alderlake)\.dll$/i.test(name)))
  throw new Error('Multi-CPU build lacks an optimised CPU backend alongside the x64 baseline.');
const files = [];
function walk(directory) {
  for (const entry of fs.readdirSync(directory, {withFileTypes:true})) {
    const absolute = path.join(directory,entry.name);
    if (entry.isSymbolicLink()) throw new Error(`Unexpected symlink: ${absolute}`);
    if (entry.isDirectory()) walk(absolute);
    else if (/\.(exe|dll)$/i.test(entry.name)) files.push(absolute);
    else if (!/\.(json|txt|md)$/i.test(entry.name)) throw new Error(`Unexpected runtime payload: ${absolute}`);
  }
}
walk(root);
const appExe = option('--app-exe'); if (appExe) files.push(path.resolve(appExe));
const records = [];
for (const file of files) {
  const data = fs.readFileSync(file); const info = assertX64(data, file);
  const dependencies = [];
  if (dllCheck) {
    const siblings = new Map(fs.readdirSync(path.dirname(file)).map(n => [n.toLowerCase(), n]));
    for (const imported of info.imports) {
      if (requiresSeparateMsvcRuntime(imported)) throw new Error(`${file} requires ${imported}; static CRT/OpenMP settings were not applied.`);
      const sibling = siblings.get(imported.toLowerCase());
      if (sibling) dependencies.push({name:imported, from:'beside_binary'});
      else if (isApiSet(imported)) dependencies.push({name:imported, from:'windows_api_contract'});
      else if (fs.existsSync(path.join(process.env.SystemRoot || 'C:\\Windows','System32',imported))) dependencies.push({name:imported, from:'windows_system32'});
      else throw new Error(`${file} imports missing ${imported}; package the matching x64 DLL, or correct the native build. Do not copy a DLL from an arbitrary website.`);
    }
  }
  records.push({file:path.relative(root,file).replaceAll('\\','/'), bytes:data.length, architecture:info.architecture,
    sha256:crypto.createHash('sha256').update(data).digest('hex'), imports:info.imports, dependencies});
}
let version = null;
if (smoke) {
  // --version only. The capture helper has no --help mode and must not be launched by build validation.
  const server = path.join(root,'qwen','llama-server.exe');
  version = execFileSync(server,['--version'],{encoding:'utf8', cwd:path.dirname(server), windowsHide:true, timeout:30000, maxBuffer:2*1024*1024});
}
const report = {schema_version:1, checked_utc:new Date().toISOString(), cpu_profile:profile,
  pe_headers:'passed', dll_closure:dllCheck?'passed':'not_run', server_version_smoke:smoke?'passed':'not_run',
  installer_stub_architecture:'not_checked_by_design', recording:'not_run', inference:'not_run', version, records};
const out = option('--out'); if (out) { fs.mkdirSync(path.dirname(path.resolve(out)),{recursive:true}); fs.writeFileSync(out,JSON.stringify(report,null,2)+'\n'); }
console.log(JSON.stringify(report,null,2));
