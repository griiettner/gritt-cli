import path from 'node:path';
import { CliError, runProcess } from './cli.mjs';
import { exists, mkdir, readText, writeText } from './fs-utils.mjs';
import { SHARED_NAMESPACE, isNamespaceName } from './tkt-store.mjs';

export const IDENTITY_ENV = 'GRITT_TKT_NAMESPACE';
export const IDENTITY_RELATIVE_PATH = path.join(
  '.agents',
  'state',
  'identity.local.yaml',
);

export function identityFilePath(repoRoot) {
  return path.join(repoRoot, IDENTITY_RELATIVE_PATH);
}

export function parseIdentityYaml(text) {
  const login = text.match(/^github_login:\s*(\S+)\s*$/m)?.[1];
  if (!login) return null;
  return {
    github_login: login,
    source: text.match(/^source:\s*(\S+)\s*$/m)?.[1] || 'file',
    resolved_at: text.match(/^resolved_at:\s*(\S+)\s*$/m)?.[1] || '',
  };
}

export function renderIdentityYaml(identity) {
  return [
    `github_login: ${identity.github_login}`,
    `source: ${identity.source}`,
    `resolved_at: ${identity.resolved_at}`,
    '',
  ].join('\n');
}

export function normalizeNamespace(value, label = 'namespace') {
  const namespace = String(value || '').trim();
  if (!isNamespaceName(namespace) || namespace === SHARED_NAMESPACE) {
    throw new CliError(
      `invalid ${label}: ${value}. Use a GitHub login (letters, digits, hyphen, underscore, dot).`,
    );
  }
  return namespace;
}

export async function persistIdentity(repoRoot, identity) {
  const target = identityFilePath(repoRoot);
  await mkdir(path.dirname(target), { recursive: true });
  await writeText(target, renderIdentityYaml(identity));
  return target;
}

export async function resolveTicketIdentity(repoRoot, options = {}) {
  const override = options.namespace?.trim();
  if (override) {
    const identity = {
      github_login: normalizeNamespace(override, '--namespace'),
      source: 'flag',
      resolved_at: new Date().toISOString(),
    };
    if (options.persist !== false) await persistIdentity(repoRoot, identity);
    return identity;
  }

  const fromEnv = process.env[IDENTITY_ENV]?.trim();
  if (fromEnv) {
    const identity = {
      github_login: normalizeNamespace(fromEnv, IDENTITY_ENV),
      source: 'env',
      resolved_at: new Date().toISOString(),
    };
    if (options.persist !== false) await persistIdentity(repoRoot, identity);
    return identity;
  }

  if (!options.refresh) {
    const stored = await readStoredIdentity(repoRoot);
    if (stored) return stored;
  }

  const fromGh = lookupGithubLogin(repoRoot);
  if (fromGh) {
    const identity = {
      github_login: normalizeNamespace(fromGh, 'GitHub login'),
      source: 'gh',
      resolved_at: new Date().toISOString(),
    };
    if (options.persist !== false) await persistIdentity(repoRoot, identity);
    return identity;
  }

  throw new CliError(
    [
      'could not resolve a GitHub login for ticket namespacing.',
      'Run `gh auth login`, set GRITT_TKT_NAMESPACE, or pass --namespace <github-login>.',
    ].join(' '),
  );
}

async function readStoredIdentity(repoRoot) {
  const target = identityFilePath(repoRoot);
  if (!(await exists(target))) return null;
  const parsed = parseIdentityYaml(await readText(target));
  if (!parsed) return null;
  parsed.github_login = normalizeNamespace(
    parsed.github_login,
    IDENTITY_RELATIVE_PATH,
  );
  return parsed;
}

function lookupGithubLogin(repoRoot) {
  const result = runProcess('gh', ['api', 'user', '--jq', '.login'], {
    cwd: repoRoot,
  });
  if (result.status !== 0) return '';
  return result.stdout.trim();
}
