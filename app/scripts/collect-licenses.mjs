import fs from 'node:fs';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
const app = path.resolve(import.meta.dirname, '..');
const destination = path.join(app, 'src-tauri/resources/licenses');
fs.mkdirSync(destination, {recursive: true});
const records = [];
const seen = new Set();
function add(name, version, directory, license, repository) {
  const key = `${name}@${version}`;
  if (seen.has(key)) return;
  seen.add(key);
  const prefix = `${name}-${version}`.replaceAll(/[^A-Za-z0-9._-]/g, '_');
  const files = fs.readdirSync(directory).filter(file => /^(licen[cs]e|copying|notice)([._-]|$)/i.test(file) && fs.statSync(path.join(directory,file)).isFile());
  for (const file of files) fs.copyFileSync(path.join(directory,file), path.join(destination, `${prefix}-${file}`));
  records.push({name,version,license,repository,notices:files.map(file=>`${prefix}-${file}`)});
}
const lock = JSON.parse(fs.readFileSync(path.join(app, 'package-lock.json'), 'utf8'));
for (const [entry, info] of Object.entries(lock.packages)) {
  if (!entry || info.dev) continue;
  const directory = path.join(app, entry);
  const manifest = JSON.parse(fs.readFileSync(path.join(directory, 'package.json'), 'utf8'));
  add(manifest.name, manifest.version, directory, manifest.license, typeof manifest.repository==='string'?manifest.repository:manifest.repository?.url);
}
const metadata = JSON.parse(execFileSync('cargo',['metadata','--locked','--format-version','1'], {cwd:path.join(app,'src-tauri'),encoding:'utf8',maxBuffer:16*1024*1024}));
for (const pkg of metadata.packages) {
  if (pkg.source) add(pkg.name,pkg.version,path.dirname(pkg.manifest_path),pkg.license,pkg.repository);
}
if (process.platform === 'win32') {
  const helperMetadata = JSON.parse(execFileSync('cargo',['metadata','--locked','--format-version','1'], {cwd:path.join(app,'native/windows-loopback'),encoding:'utf8',maxBuffer:16*1024*1024}));
  for (const pkg of helperMetadata.packages) if (pkg.source) add(pkg.name,pkg.version,path.dirname(pkg.manifest_path),pkg.license,pkg.repository);
}
add('llama.cpp','391fac16460f15233a7740550d858ac96df3419d',path.join(app,'.native-build/source/llama'),'MIT','https://github.com/ggml-org/llama.cpp');
fs.writeFileSync(path.join(destination,'DEPENDENCIES.json'),JSON.stringify(records,null,2)+'\n');
fs.writeFileSync(path.join(destination,'README.txt'),'Suitang (随堂) includes the open-source components listed in DEPENDENCIES.json. Copyright and license notices are included in this directory. Model weights are downloaded separately; their license is available from the model publisher.\n');
process.stdout.write(`Collected notices for ${records.length} dependencies.\n`);
