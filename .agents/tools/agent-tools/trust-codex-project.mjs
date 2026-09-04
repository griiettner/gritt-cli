import os from 'node:os';
import path from 'node:path';
import { parseCli, runMain, runProcess } from './lib/cli.mjs';
import { exists, readText, writeText } from './lib/fs-utils.mjs';

function usage() {
  return `usage: trust-codex-project.mjs [-h] [--check] [path]

Check or add the exact Codex trust entry for this repository.

positional arguments:
  path        Repository path to trust. Defaults to the current directory.

options:
  -h, --help  show this help message and exit
  --check     Only check trust state. Exit 0 when trusted, 1 when not trusted.`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), { check: { type: 'boolean' } }, [
    { name: 'path', required: false, default: '.' },
  ]);
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const projectPath = resolveRepoRoot(args.path);
  const configPath = codexConfigPath();
  if (args.check) {
    if (await isTrusted(configPath, projectPath)) {
      console.log(`trusted: ${projectPath}`);
      return 0;
    }
    console.log(`not trusted: ${projectPath}`);
    console.log(`config: ${configPath}`);
    return 1;
  }
  const changed = await ensureTrusted(configPath, projectPath);
  console.log(`${changed ? 'trusted' : 'already trusted'}: ${projectPath}`);
  console.log(`config: ${configPath}`);
  if (changed) {
    console.log(
      'restart required: start a fresh Codex session at this repository root',
    );
  }
  return 0;
}

function resolveRepoRoot(start) {
  const resolved = path.resolve(start);
  const result = runProcess('git', [
    '-C',
    resolved,
    'rev-parse',
    '--show-toplevel',
  ]);
  return result.status === 0 ? path.resolve(result.stdout.trim()) : resolved;
}

function codexConfigPath() {
  const codexHome = expandHome(
    process.env.CODEX_HOME || path.join(os.homedir(), '.codex'),
  );
  return path.join(codexHome, 'config.toml');
}

function expandHome(value) {
  if (value === '~') return os.homedir();
  if (value.startsWith(`~${path.sep}`))
    return path.join(os.homedir(), value.slice(2));
  return path.resolve(value);
}

function projectHeader(projectPath) {
  return `[projects."${tomlBasicString(projectPath)}"]`;
}

function tomlBasicString(value) {
  return String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function findSection(lines, header) {
  const start = lines.findIndex((line) => line.trim() === header);
  if (start === -1) return null;
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    const stripped = lines[index].trim();
    if (stripped.startsWith('[') && stripped.endsWith(']')) {
      end = index;
      break;
    }
  }
  return [start, end];
}

function sectionIsTrusted(lines, section) {
  const [start, end] = section;
  return lines
    .slice(start + 1, end)
    .some((line) => line.trim() === 'trust_level = "trusted"');
}

async function ensureTrusted(configPath, projectPath) {
  const header = projectHeader(projectPath);
  const text = await readText(configPath, '');
  const lines = text.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  const section = findSection(lines, header);
  if (!section) {
    if (lines.length && lines.at(-1).trim()) lines.push('');
    lines.push(header, 'trust_level = "trusted"');
    await writeText(configPath, `${lines.join('\n')}\n`);
    return true;
  }
  if (sectionIsTrusted(lines, section)) return false;
  const [start, end] = section;
  const trustIndex = lines.findIndex(
    (line, index) =>
      index > start && index < end && line.trim().startsWith('trust_level'),
  );
  if (trustIndex === -1) lines.splice(start + 1, 0, 'trust_level = "trusted"');
  else lines[trustIndex] = 'trust_level = "trusted"';
  await writeText(configPath, `${lines.join('\n')}\n`);
  return true;
}

async function isTrusted(configPath, projectPath) {
  if (!(await exists(configPath))) return false;
  const lines = (await readText(configPath)).split(/\r?\n/);
  const section = findSection(lines, projectHeader(projectPath));
  return Boolean(section && sectionIsTrusted(lines, section));
}

await runMain(main);
