// Native Pi oracle for optional differential tests; NOT a runtime adapter/plugin.
// Run only against the explicitly provided installed Pi 0.87.1 package.
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const pkg = resolve(process.argv[2]);
const manifest = JSON.parse(await readFile(join(pkg, 'package.json'), 'utf8'));
if (manifest.version !== '0.87.1') throw new Error('Reference must be Pi 0.87.1');
const pi = await import(pathToFileURL(join(pkg, 'dist/index.js')).href);
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const cases = JSON.parse(Buffer.concat(chunks).toString());
const results = [];
for (const c of cases) {
  const cwd = await mkdtemp(join(tmpdir(), 'portspace-pi-reference-'));
  try {
    for (const [path, content] of Object.entries(c.files)) {
      await mkdir(dirname(join(cwd, path)), { recursive: true });
      await writeFile(join(cwd, path), content);
    }
    const tools = {
      read: pi.createReadToolDefinition(cwd),
      write: pi.createWriteToolDefinition(cwd),
      edit: pi.createEditToolDefinition(cwd),
    };
    let result;
    try {
      const r = await tools[c.tool].execute('reference', c.args, new AbortController().signal);
      result = { isError: false, text: r.content.filter(x => x.type === 'text').map(x => x.text).join('\n') };
    } catch (e) { result = { isError: true, text: String(e.message) }; }
    result.files = {};
    for (const path of new Set([...Object.keys(c.files), c.args.path])) {
      try { result.files[path] = await readFile(join(cwd, path), 'utf8'); }
      catch (e) { if (e.code !== 'ENOENT') throw e; }
    }
    results.push(result);
  } finally { await rm(cwd, { recursive: true, force: true }); }
}
process.stdout.write(JSON.stringify(results));
