import fs from 'node:fs';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
if(!['win32','darwin'].includes(process.platform)) throw new Error('Native packaging requires Windows or macOS.');
const root=path.resolve(import.meta.dirname,'..');
const target=path.join(root,'src-tauri/resources/native');
fs.mkdirSync(path.join(target,'qwen'),{recursive:true});
const source=process.env.LECTUREEDIT_LLAMA_BIN || path.join(root,'.native-build/llama/bin');
const windows=process.platform==='win32';
const nonportableAbsolute=value=>value.startsWith('/Users/')||value.startsWith('/opt/')||value.startsWith('/usr/local/');
function rpaths(file){
 const lines=execFileSync('otool',['-l',file],{encoding:'utf8'}).split('\n');
 const values=[];
 for(let index=0;index<lines.length;index++){
  if(lines[index].trim()!=='cmd LC_RPATH')continue;
  for(let detail=index+1;detail<Math.min(index+5,lines.length);detail++){
   const match=lines[detail].match(/^\s*path (.+?) \(offset \d+\)\s*$/);
   if(match){values.push(match[1]);break;}
  }
 }
 return values;
}
for(const name of fs.readdirSync(source)) {
 if(windows ? (name==='llama-server.exe'||name.endsWith('.dll')) : (name==='llama-server'||name.endsWith('.dylib')))fs.copyFileSync(path.join(source,name),path.join(target,'qwen',name));
}
const helper=windows?'lectureedit-loopback.exe':'lectureedit-capture';
fs.copyFileSync(path.join(root,'native/bin',helper),path.join(target,helper));
if(!windows){
 fs.chmodSync(path.join(target,helper),0o755);
 for(const name of fs.readdirSync(path.join(target,'qwen'))){
  const file=path.join(target,'qwen',name);
  fs.chmodSync(file,0o755);
  const deps=execFileSync('otool',['-L',file],{encoding:'utf8'}).split('\n').slice(1).map(x=>x.trim().split(' ')[0]).filter(Boolean);
  for(const dep of deps){if(dep.startsWith('@rpath/'))execFileSync('install_name_tool',['-change',dep,'@loader_path/'+path.basename(dep),file]);else if(nonportableAbsolute(dep))throw new Error(`Nonportable dependency ${dep} in ${name}`);}
  for(const rpath of rpaths(file)){if(nonportableAbsolute(rpath))execFileSync('install_name_tool',['-delete_rpath',rpath,file]);}
  const remaining=rpaths(file).find(nonportableAbsolute);
  if(remaining)throw new Error(`Nonportable LC_RPATH ${remaining} in ${name}`);
  execFileSync('codesign',['--force','--sign','-',file],{stdio:'pipe'});
 }
 execFileSync('codesign',['--force','--sign','-',path.join(target,helper)],{stdio:'pipe'});
}
console.log(`Native runtime packaged: ${target}`);
