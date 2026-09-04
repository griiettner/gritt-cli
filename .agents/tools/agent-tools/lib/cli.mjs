import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';

export class CliError extends Error {
  constructor(message, exitCode = 2) {
    super(message);
    this.exitCode = exitCode;
  }
}

export function parseCli(argv, definitions = {}, positionals = []) {
  const values = {};
  const positionalValues = [];
  const seenMulti = new Set();

  for (const [name, definition] of Object.entries(definitions)) {
    if (Object.hasOwn(definition, 'default')) {
      values[name] = Array.isArray(definition.default)
        ? [...definition.default]
        : definition.default;
    } else if (definition.type === 'boolean') {
      values[name] = false;
    }
  }

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === '--help' || token === '-h') {
      values.help = true;
      continue;
    }
    if (token === '--') {
      positionalValues.push(...argv.slice(index + 1));
      break;
    }
    if (!token.startsWith('--')) {
      positionalValues.push(token);
      continue;
    }

    const equals = token.indexOf('=');
    const name = token.slice(2, equals === -1 ? undefined : equals);
    const definition = definitions[name];
    if (!definition) {
      throw new CliError(`unrecognized arguments: --${name}`);
    }
    if (definition.type === 'boolean') {
      if (equals !== -1) {
        throw new CliError(`argument --${name}: ignored explicit argument`);
      }
      values[name] = true;
      continue;
    }

    if (definition.type === 'multi') {
      const items = [];
      if (equals !== -1) {
        items.push(token.slice(equals + 1));
      } else {
        while (index + 1 < argv.length && !argv[index + 1].startsWith('--')) {
          items.push(argv[index + 1]);
          index += 1;
        }
      }
      // First occurrence replaces the default; repeats append.
      values[name] = seenMulti.has(name) ? [...values[name], ...items] : items;
      seenMulti.add(name);
      continue;
    }

    const value = equals === -1 ? argv[index + 1] : token.slice(equals + 1);
    if (value === undefined || (equals === -1 && value.startsWith('--'))) {
      throw new CliError(`argument --${name}: expected one argument`);
    }
    if (equals === -1) {
      index += 1;
    }
    values[name] = value;
  }

  for (let index = 0; index < positionals.length; index += 1) {
    const definition = positionals[index];
    const value = positionalValues[index];
    if (value === undefined && definition.required !== false && !values.help) {
      throw new CliError(
        `the following arguments are required: ${definition.name}`,
      );
    }
    values[definition.name] = value ?? definition.default;
  }
  if (positionalValues.length > positionals.length) {
    throw new CliError(
      `unrecognized arguments: ${positionalValues.slice(positionals.length).join(' ')}`,
    );
  }

  if (!values.help) {
    for (const [name, definition] of Object.entries(definitions)) {
      if (definition.required && values[name] === undefined) {
        throw new CliError(`the following arguments are required: --${name}`);
      }
    }
  }
  return values;
}

export function runProcess(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    encoding: 'utf8',
    env: options.env,
    stdio: options.inherit ? 'inherit' : 'pipe',
  });
  return {
    status: result.status ?? 1,
    stdout: result.stdout?.toString() ?? '',
    stderr: result.stderr?.toString() ?? result.error?.message ?? '',
  };
}

function resolveAgentCli(repo) {
  const fromEnv = process.env.GRITT_AGENT_BIN?.trim();
  if (fromEnv) return { command: fromEnv, prefix: [] };
  const binary = process.platform === 'win32' ? 'gritt-agent.exe' : 'gritt-agent';
  for (const profile of ['release', 'debug']) {
    const candidate = path.join(repo, '.agents', 'cli', 'target', profile, binary);
    if (pathExistsSync(candidate)) return { command: candidate, prefix: [] };
  }
  return {
    command: 'cargo',
    prefix: [
      'run',
      '--quiet',
      '--release',
      '--manifest-path',
      path.join(repo, '.agents', 'cli', 'Cargo.toml'),
      '--',
    ],
  };
}

/**
 * Runs a `gritt-agent` subcommand such as `['ticket', 'sync']`.
 * Binary resolution order: GRITT_AGENT_BIN, the release build, the debug
 * build, then `cargo run` against `.agents/cli/Cargo.toml`.
 */
export function runAgentCli(repo, subcommand, options = {}) {
  const { command, prefix } = resolveAgentCli(repo);
  const result = runProcess(
    command,
    [...prefix, '--repo-root', repo, ...subcommand],
    { cwd: repo, inherit: options.inherit },
  );
  if (result.status !== 0 && options.inherit && result.stderr) {
    // With inherited stdio only a spawn failure lands here; do not lose it.
    console.error(`error: could not run ${command}: ${result.stderr.trim()}`);
  }
  return result;
}

function pathExistsSync(target) {
  return existsSync(target);
}

export async function runMain(main) {
  try {
    process.exitCode = await main();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(error instanceof CliError ? `error: ${message}` : message);
    process.exitCode = error instanceof CliError ? error.exitCode : 1;
  }
}
