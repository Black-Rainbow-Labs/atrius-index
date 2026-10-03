#!/usr/bin/env node
// Clones every corpus in tests/corpora.json at its tag into ATRIUS_CORPORA, one folder per corpus.
//
//   node tests/fetch-corpora.mjs                 every corpus
//   node tests/fetch-corpora.mjs small medium    only these sizes ("small", "medium", "large", "extra large" as extra-large)
//   node tests/fetch-corpora.mjs --only redis    one corpus
//
// A corpus already present at the right tag is left alone. Each clone is shallow, at the tag, and keeps
// its .git folder, because ripgrep reads .gitignore only inside a git repository and the tests compare
// with ripgrep. `corpora.lock.json` in the corpora folder records the commit and file count of each one.
// ATRIUS_LINUX names an existing Linux tree to use instead of cloning one (about 1.5 GB).

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import os from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(readFileSync(join(here, 'corpora.json'), 'utf8'));

/** The folder the corpora live in: ATRIUS_CORPORA, D:/atrius-corpora when D: exists on Windows, or ~/.cache/atrius-corpora. */
export function corporaRoot() {
  if (process.env.ATRIUS_CORPORA) return process.env.ATRIUS_CORPORA;
  if (process.platform === 'win32' && existsSync('D:/')) return 'D:/atrius-corpora';
  return join(os.homedir(), '.cache', 'atrius-corpora');
}

/** Runs git with arguments and answers its standard output, trimmed. */
function git(args, cwd) {
  return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

/** Counts the files under a folder, leaving out .git. */
function countFiles(folder) {
  let count = 0;
  const stack = [folder];
  while (stack.length) {
    const dir = stack.pop();
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.name === '.git') continue;
      const path = join(dir, entry.name);
      if (entry.isDirectory()) stack.push(path);
      else if (entry.isFile()) count++;
    }
  }
  return count;
}

/** Clones one corpus at its tag unless it is already there, and answers its lock entry. */
function fetchOne(root, corpus) {
  const folder = join(root, corpus.name);
  if (corpus.name === 'linux' && process.env.ATRIUS_LINUX && existsSync(process.env.ATRIUS_LINUX)) {
    return { name: corpus.name, folder: process.env.ATRIUS_LINUX, tag: corpus.tag, commit: 'external', files: countFiles(process.env.ATRIUS_LINUX) };
  }
  const marker = join(folder, '.atrius-tag');
  if (existsSync(marker) && readFileSync(marker, 'utf8').trim() === corpus.tag) {
    return { name: corpus.name, folder, tag: corpus.tag, commit: git(['rev-parse', 'HEAD'], folder), files: countFiles(folder) };
  }
  if (existsSync(folder) && readdirSync(folder).length) {
    throw new Error(`${folder} exists and is not ${corpus.repo} at ${corpus.tag}. Move it away and run again.`);
  }
  console.log(`cloning ${corpus.repo} at ${corpus.tag}`);
  execFileSync('git', ['-c', 'core.autocrlf=false', 'clone', '--quiet', '--depth', '1', '--branch', corpus.tag, `https://github.com/${corpus.repo}.git`, folder], { stdio: 'inherit' });
  writeFileSync(marker, corpus.tag + '\n');
  return { name: corpus.name, folder, tag: corpus.tag, commit: git(['rev-parse', 'HEAD'], folder), files: countFiles(folder) };
}

function main() {
  const args = process.argv.slice(2);
  const only = args.includes('--only') ? args[args.indexOf('--only') + 1] : null;
  const sizes = args.filter((a) => !a.startsWith('--') && a !== only).map((s) => s.replace('-', ' '));
  const root = corporaRoot();
  mkdirSync(root, { recursive: true });
  const lockPath = join(root, 'corpora.lock.json');
  const lock = existsSync(lockPath) ? JSON.parse(readFileSync(lockPath, 'utf8')) : {};
  for (const corpus of manifest.corpora) {
    if (only && corpus.name !== only) continue;
    if (sizes.length && !sizes.includes(corpus.size)) continue;
    const entry = fetchOne(root, corpus);
    lock[corpus.name] = entry;
    console.log(`${corpus.name.padEnd(16)} ${corpus.language.padEnd(11)} ${String(entry.files).padStart(7)} files  ${entry.commit.slice(0, 12)}`);
    writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
else if (process.argv[1] && process.argv[1].endsWith('fetch-corpora.mjs')) main();
