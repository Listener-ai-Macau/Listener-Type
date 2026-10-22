// Run every source TypeScript unit file, including newly added feature tests.
import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

function testsIn(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? testsIn(path)
      : entry.isFile() && entry.name.endsWith('.test.ts') ? [path] : [];
  });
}
const tests = testsIn(fileURLToPath(new URL('../src/', import.meta.url))).sort();
if (!tests.length) throw new Error('No frontend unit tests were discovered');
console.log(`Running all ${tests.length} frontend TypeScript unit files`);
const result = spawnSync(process.execPath, [
  '--experimental-strip-types', '--test', '--test-concurrency=1', ...tests,
], { stdio: 'inherit', timeout: 120_000 });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
