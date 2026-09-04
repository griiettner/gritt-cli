import {
  access,
  mkdir,
  readdir,
  readFile,
  realpath,
  rm,
  stat,
  unlink,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';
import { runProcess } from './cli.mjs';

export async function exists(target) {
  try {
    await access(target);
    return true;
  } catch {
    return false;
  }
}

export async function isDirectory(target) {
  try {
    return (await stat(target)).isDirectory();
  } catch {
    return false;
  }
}

export async function isFile(target) {
  try {
    return (await stat(target)).isFile();
  } catch {
    return false;
  }
}

export async function listEntries(target) {
  return (await readdir(target, { withFileTypes: true })).sort((a, b) =>
    a.name.localeCompare(b.name),
  );
}

export async function listDirectories(target) {
  return (await listEntries(target)).filter((entry) => entry.isDirectory());
}

export async function listFilesRecursive(target) {
  const files = [];
  if (!(await isDirectory(target))) {
    return files;
  }
  for (const entry of await listEntries(target)) {
    const child = path.join(target, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFilesRecursive(child)));
    } else if (entry.isFile()) {
      files.push(child);
    }
  }
  return files;
}

export async function readText(target, fallback) {
  try {
    return await readFile(target, 'utf8');
  } catch (error) {
    if (fallback !== undefined && error?.code === 'ENOENT') {
      return fallback;
    }
    throw error;
  }
}

export async function writeText(target, content) {
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, content, 'utf8');
}

export async function removeFile(target) {
  await unlink(target);
}

export async function removeDirectory(target) {
  await rm(target, { recursive: true, force: true });
}

export function relativePosix(root, target) {
  const value = path.relative(root, target);
  return value.startsWith('..') ? target : value.split(path.sep).join('/');
}

export async function resolveExisting(target) {
  try {
    return await realpath(target);
  } catch {
    return path.resolve(target);
  }
}

export async function resolveRepoRoot(start) {
  const resolved = path.resolve(start);
  const result = runProcess('git', [
    '-C',
    resolved,
    'rev-parse',
    '--show-toplevel',
  ]);
  return result.status === 0 ? path.resolve(result.stdout.trim()) : resolved;
}

export function localDate() {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, '0');
  const day = String(now.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export { mkdir, path, stat };
