#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import process from 'node:process';

function runGit(args) {
  const result = spawnSync('git', args, {
    cwd: process.cwd(),
    encoding: 'utf8',
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || '').trim();
    throw new Error(`git ${args.join(' ')} failed${detail ? `: ${detail}` : ''}`);
  }
  return result.stdout.replace(/\r\n/g, '\n').trimEnd();
}

const status = runGit(['status', '--porcelain=v1', '--untracked-files=all']);
if (status.trim().length > 0) {
  const lines = status.split('\n').filter(Boolean);
  const preview = lines.slice(0, 80).join('\n');
  const hidden = lines.length > 80 ? `\n... ${lines.length - 80} more changed paths hidden` : '';
  console.error('FAIL: release/review hygiene requires a clean Listener Type worktree.');
  console.error('Commit accepted changes, stash/delete unrelated work, or split pending work before packaging or final review.');
  console.error(`Dirty paths (${lines.length}):\n${preview}${hidden}`);
  process.exit(1);
}

const diffCheck = spawnSync('git', ['diff', '--check'], {
  cwd: process.cwd(),
  encoding: 'utf8',
});
if (diffCheck.error) {
  throw diffCheck.error;
}
if (diffCheck.status !== 0) {
  console.error('FAIL: git diff --check found whitespace or conflict-marker issues.');
  process.stderr.write(diffCheck.stdout || diffCheck.stderr || '');
  process.exit(diffCheck.status ?? 1);
}

console.log('PASS: Listener Type worktree is clean for release/review hygiene.');
