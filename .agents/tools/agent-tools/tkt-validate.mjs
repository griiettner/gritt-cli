import path from 'node:path';
import { parseCli, runMain } from './lib/cli.mjs';
import {
  exists,
  isDirectory,
  listDirectories,
  listEntries,
  readText,
} from './lib/fs-utils.mjs';
import { LIST_FIELDS, loadFrontmatter } from './frontmatter-utils.mjs';
import {
  SHARED_NAMESPACE,
  chunkName,
  isChunkDirName,
  isNamespaceName,
  isTicketId,
  listNamespaces,
  qualifiedTicketId,
  ticketNumber,
} from './lib/tkt-store.mjs';

const KNOWN_ARTIFACTS = ['concept', 'plan', 'task', 'report'];
const REQUIRED_FIELDS = [
  'id',
  'title',
  'artifact',
  'status',
  'created',
  'updated',
];
const INDEX_PATH_RE = /\.agents\/tasks\/[^\s,\]}]+/g;
const SHARD_PATH_RE =
  /\.agents\/tasks\/(?:[A-Za-z0-9._-]+\/)?TKT-\d{4}-\d{4}\/index\.yaml/g;
const ALLOWED_STATUSES = new Set([
  'concept',
  'planning',
  'ready',
  'in_progress',
  'done',
  'blocked',
  'cancelled',
]);
const ALLOWED_ARTIFACTS = new Set([
  'concept',
  'plan',
  'task',
  'report',
  'update',
]);
const ALLOWED_CHAIN_ROLES = new Set(['orchestrator', 'worker', 'reviewer']);
const SCAFFOLD_MARKER = 'TODO(tkt):';

class Outcome {
  errors = [];
  warnings = [];

  error(message) {
    this.errors.push(message);
  }

  warn(message) {
    this.warnings.push(message);
  }
}

function usage() {
  return `usage: tkt-validate.mjs [-h] [root]

Validate agent ticket folders without external dependencies.

positional arguments:
  root        Tasks root (default: .agents/tasks)

options:
  -h, --help  show this help message and exit`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {}, [
    { name: 'root', required: false, default: '.agents/tasks' },
  ]);
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const root = path.resolve(args.root);
  const repoRoot =
    path.basename(root) === 'tasks' &&
    path.basename(path.dirname(root)) === '.agents'
      ? path.dirname(path.dirname(root))
      : path.dirname(root);
  const outcome = new Outcome();
  if (!(await isDirectory(root))) {
    outcome.error(`tasks root does not exist: ${root}`);
    return finish(outcome);
  }
  const chains = new Map();
  const ticketIds = await validateTicketFolders(root, repoRoot, outcome, chains);
  validateChains(chains, outcome);
  await validateMemoryFrontmatter(
    path.join(repoRoot, '.agents', 'memory'),
    outcome,
  );
  await validateOptionalIndex(root, repoRoot, ticketIds, outcome);
  return finish(outcome);
}

async function validateTicketFolders(root, repoRoot, outcome, chains) {
  const ticketIds = new Set();
  for (const namespace of await listNamespaces(root)) {
    if (namespace.id !== SHARED_NAMESPACE && !(await isDirectory(namespace.root))) {
      continue;
    }
    if (!(await isDirectory(namespace.root))) continue;
    for (const chunk of await listDirectories(namespace.root)) {
      const chunkPath = path.join(namespace.root, chunk.name);
      if (namespace.id === SHARED_NAMESPACE && isNamespaceName(chunk.name)) {
        continue;
      }
      if (!isChunkDirName(chunk.name)) {
        outcome.error(`invalid chunk folder name: ${chunkPath}`);
        continue;
      }
      await validateChunkFolder(
        chunkPath,
        chunk.name,
        namespace.id,
        ticketIds,
        outcome,
        chains,
      );
    }
  }
  return ticketIds;
}

async function validateChunkFolder(
  chunkPath,
  chunkDirName,
  namespace,
  ticketIds,
  outcome,
  chains,
) {
  for (const entry of await listEntries(chunkPath)) {
    const target = path.join(chunkPath, entry.name);
    if (entry.isFile()) {
      if (entry.name !== 'index.yaml') {
        outcome.warn(`unexpected file in chunk folder: ${target}`);
      }
      continue;
    }
    if (!entry.isDirectory()) continue;
    const ticketId = entry.name;
    if (!isTicketId(ticketId)) {
      outcome.error(`invalid ticket folder name: ${target}`);
      continue;
    }
    const expectedChunk = chunkName(ticketNumber(ticketId));
    if (expectedChunk !== chunkDirName) {
      outcome.error(
        `${qualifiedTicketId(namespace, ticketId)} is in ${chunkDirName} but belongs in ${expectedChunk}`,
      );
    }
    ticketIds.add(qualifiedTicketId(namespace, ticketId));
    await validateTicketFolder(target, ticketId, namespace, outcome, chains);
  }
}

async function validateTicketFolder(
  ticketPath,
  ticketId,
  namespace,
  outcome,
  chains,
) {
  const artifacts = new Set();
  for (const artifact of KNOWN_ARTIFACTS) {
    const artifactPath = path.join(ticketPath, `${artifact}.md`);
    if (await exists(artifactPath)) {
      artifacts.add(artifact);
      const metadata = await validateArtifact(
        artifactPath,
        ticketId,
        artifact,
        outcome,
      );
      if (artifact === 'task' && metadata?.chain_role) {
        chains.set(qualifiedTicketId(namespace, ticketId), {
          namespace,
          ticketId,
          target: artifactPath,
          role: metadata.chain_role,
          parent: metadata.chain_parent ?? null,
          children: metadata.chain_children ?? [],
        });
      }
    }
  }
  const updates = path.join(ticketPath, 'updates');
  if (await exists(updates)) {
    if (!(await isDirectory(updates))) {
      outcome.error(`updates path is not a directory: ${updates}`);
    } else {
      for (const entry of await listEntries(updates)) {
        if (entry.isFile() && entry.name.endsWith('.md')) {
          await validateArtifact(
            path.join(updates, entry.name),
            ticketId,
            'update',
            outcome,
          );
        }
      }
    }
  }
  if (!artifacts.size) {
    outcome.error(`${ticketId} has no lifecycle artifacts`);
  } else if (!artifacts.has('task')) {
    outcome.warn(
      `${ticketId} has no task.md; valid for early concepts, but not executable`,
    );
  }
}

async function validateArtifact(target, ticketId, expectedArtifact, outcome) {
  const { metadata, errors } = await loadFrontmatter(target);
  if (errors.length) {
    errors.forEach((error) => outcome.error(error.render()));
    return null;
  }
  if (!Object.keys(metadata).length) {
    outcome.error(`missing YAML frontmatter: ${target}`);
    return null;
  }
  await checkScaffoldMarkers(target, outcome);
  for (const field of REQUIRED_FIELDS) {
    if (!metadata[field]) outcome.error(`missing \`${field}\` in ${target}`);
  }
  if (metadata.id && metadata.id !== ticketId) {
    outcome.error(
      `id mismatch in ${target}: expected ${ticketId}, got ${metadata.id}`,
    );
  }
  if (
    metadata.chain_role &&
    !ALLOWED_CHAIN_ROLES.has(metadata.chain_role)
  ) {
    outcome.error(
      `unsupported chain_role in ${target}: ${metadata.chain_role}`,
    );
  }
  if (metadata.artifact && !ALLOWED_ARTIFACTS.has(metadata.artifact)) {
    outcome.error(
      `unsupported artifact value in ${target}: ${metadata.artifact}`,
    );
  }
  if (metadata.artifact && metadata.artifact !== expectedArtifact) {
    outcome.error(
      `artifact mismatch in ${target}: expected ${expectedArtifact}, got ${metadata.artifact}`,
    );
  }
  if (metadata.status && !ALLOWED_STATUSES.has(metadata.status)) {
    outcome.warn(`suspicious status value in ${target}: ${metadata.status}`);
  }
  for (const field of LIST_FIELDS) {
    const value = metadata[field];
    if (value !== undefined && !Array.isArray(value)) {
      outcome.error(`field \`${field}\` must be a list in ${target}`);
    }
  }
  return metadata;
}

async function checkScaffoldMarkers(target, outcome) {
  let content;
  try {
    content = await readText(target);
  } catch (error) {
    outcome.error(`cannot read ${target}: ${error.message}`);
    return;
  }
  const pending = content
    .split(/\r?\n/)
    .filter((line) => line.includes(SCAFFOLD_MARKER)).length;
  if (pending) {
    outcome.error(
      `${target} still has ${pending} unfilled scaffold line(s) marked \`${SCAFFOLD_MARKER}\`; replace them with real content`,
    );
  }
}

function validateChains(chains, outcome) {
  for (const entry of chains.values()) {
    const qualified = qualifiedTicketId(entry.namespace, entry.ticketId);
    if (entry.role === 'orchestrator') {
      if (!entry.children.length) {
        outcome.error(
          `${qualified} is a chain orchestrator with no \`chain_children\`; a chain is an orchestrator plus one ticket per worker step`,
        );
        continue;
      }
      for (const child of entry.children) {
        const childKey = qualifiedTicketId(entry.namespace, child);
        const record = chains.get(childKey);
        if (!record) {
          outcome.error(
            `${qualified} lists chain child ${child}, but no such chain ticket exists in namespace ${entry.namespace}`,
          );
          continue;
        }
        if (record.parent !== entry.ticketId) {
          outcome.error(
            `${childKey} has chain_parent ${record.parent ?? 'none'} but is listed as a child of ${entry.ticketId}`,
          );
        }
      }
      continue;
    }
    if (!entry.parent) {
      outcome.error(
        `${qualified} has chain_role ${entry.role} but no \`chain_parent\``,
      );
      continue;
    }
    const parentKey = qualifiedTicketId(entry.namespace, entry.parent);
    const parent = chains.get(parentKey);
    if (!parent) {
      outcome.error(
        `${qualified} points at missing chain parent ${entry.parent}`,
      );
    } else if (!parent.children.includes(entry.ticketId)) {
      outcome.error(
        `${parentKey} does not list ${entry.ticketId} in \`chain_children\``,
      );
    }
  }
}

async function validateMemoryFrontmatter(memoryRoot, outcome) {
  if (!(await isDirectory(memoryRoot))) return;
  for (const category of await listDirectories(memoryRoot)) {
    const categoryPath = path.join(memoryRoot, category.name);
    for (const entry of await listEntries(categoryPath)) {
      if (
        !entry.isFile() ||
        !entry.name.endsWith('.md') ||
        entry.name === 'index.md'
      ) {
        continue;
      }
      const target = path.join(categoryPath, entry.name);
      const { metadata, errors } = await loadFrontmatter(target);
      errors.forEach((error) => outcome.warn(error.render()));
      if (!Object.keys(metadata).length) {
        outcome.warn(`memory file is missing YAML frontmatter: ${target}`);
        continue;
      }
      if (!metadata.id)
        outcome.warn(`memory file is missing \`id\`: ${target}`);
      if (!metadata.title)
        outcome.warn(`memory file is missing \`title\`: ${target}`);
    }
  }
}

async function validateOptionalIndex(root, repoRoot, ticketIds, outcome) {
  const indexPath = path.join(root, 'index.yaml');
  if (!(await exists(indexPath))) {
    outcome.warn(
      'index.yaml is missing; this is allowed because ticket folders are source of truth',
    );
    return;
  }
  let content;
  try {
    content = await readText(indexPath);
  } catch (error) {
    outcome.warn(`cannot read optional index ${indexPath}: ${error.message}`);
    return;
  }
  const indexedIds = indexedTicketKeys(content, SHARED_NAMESPACE);
  if (matches(content, SHARD_PATH_RE).length) {
    await validateShardedIndex(repoRoot, content, ticketIds, outcome);
    return;
  }
  const hasContent = content
    .split(/\r?\n/)
    .some((line) => line.trim() && !line.trimStart().startsWith('#'));
  if (!indexedIds.size && hasContent) {
    outcome.warn('optional index.yaml has no recognizable ticket ids');
  } else if (!indexedIds.size && ticketIds.size) {
    outcome.warn(
      'index.yaml is empty but ticket folders exist; rerun `nx run agent-tools:tkt-sync`',
    );
  }
  for (const ticketId of difference(indexedIds, ticketIds)) {
    outcome.warn(
      `optional index references missing ticket folder: ${ticketId}`,
    );
  }
  for (const ticketId of difference(ticketIds, indexedIds)) {
    outcome.warn(`optional index omits existing ticket folder: ${ticketId}`);
  }
  for (const rawPath of matches(content, INDEX_PATH_RE)) {
    const normalized = rawPath.replace(/\/$/, '');
    const target = resolveIndexPath(repoRoot, normalized);
    const valid = rawPath.endsWith('/')
      ? await isDirectory(target)
      : await exists(target);
    if (!valid)
      outcome.warn(
        `optional index references missing artifact path: ${rawPath}`,
      );
  }
}

async function validateShardedIndex(repoRoot, content, ticketIds, outcome) {
  const shardPaths = matches(content, SHARD_PATH_RE);
  if (!shardPaths.length) {
    outcome.warn('optional sharded index.yaml has no recognizable shard paths');
    return;
  }
  const indexedIds = new Set();
  for (const rawPath of shardPaths) {
    const shardPath = resolveIndexPath(repoRoot, rawPath);
    if (!(await exists(shardPath))) {
      outcome.warn(`optional index references missing shard path: ${rawPath}`);
      continue;
    }
    let shardContent;
    try {
      shardContent = await readText(shardPath);
    } catch (error) {
      outcome.warn(`cannot read optional shard ${rawPath}: ${error.message}`);
      continue;
    }
    indexedTicketKeys(shardContent, namespaceFromShardPath(rawPath)).forEach(
      (id) => indexedIds.add(id),
    );
    for (const artifactPath of matches(shardContent, INDEX_PATH_RE)) {
      const normalized = artifactPath.replace(/\/$/, '');
      const target = resolveIndexPath(repoRoot, normalized);
      const valid = artifactPath.endsWith('/')
        ? await isDirectory(target)
        : await exists(target);
      if (!valid) {
        outcome.warn(
          `optional shard references missing artifact path: ${artifactPath}`,
        );
      }
    }
  }
  for (const ticketId of difference(indexedIds, ticketIds)) {
    outcome.warn(
      `optional shard index references missing ticket folder: ${ticketId}`,
    );
  }
  for (const ticketId of difference(ticketIds, indexedIds)) {
    outcome.warn(
      `optional shard indexes omit existing ticket folder: ${ticketId}`,
    );
  }
}

function indexedTicketKeys(content, defaultNamespace) {
  const keys = new Set();
  let currentId = null;
  for (const line of content.split(/\r?\n/)) {
    const idMatch = line.match(/^\s*- id:\s+(TKT-\d{4})\s*$/);
    if (idMatch) {
      if (currentId) {
        keys.add(qualifiedTicketId(defaultNamespace, currentId));
      }
      currentId = idMatch[1];
      continue;
    }
    const namespaceMatch = line.match(/^\s+namespace:\s+(\S+)\s*$/);
    if (namespaceMatch && currentId) {
      keys.add(qualifiedTicketId(namespaceMatch[1], currentId));
      currentId = null;
    }
  }
  if (currentId) keys.add(qualifiedTicketId(defaultNamespace, currentId));
  return keys;
}

function namespaceFromShardPath(rawPath) {
  const match = rawPath.match(
    /\.agents\/tasks\/(?:([^/]+)\/)?TKT-\d{4}-\d{4}\/index\.yaml/,
  );
  const maybeNamespace = match?.[1];
  if (!maybeNamespace || isChunkDirName(maybeNamespace)) {
    return SHARED_NAMESPACE;
  }
  return maybeNamespace;
}

function matches(content, regex, group = 0) {
  regex.lastIndex = 0;
  return [...content.matchAll(regex)].map((match) => match[group]);
}

function difference(left, right) {
  return [...left].filter((value) => !right.has(value)).sort();
}

function resolveIndexPath(repoRoot, rawPath) {
  return path.isAbsolute(rawPath) ? rawPath : path.join(repoRoot, rawPath);
}

function finish(outcome) {
  outcome.warnings.forEach((warning) => console.error(`warning: ${warning}`));
  if (!outcome.errors.length) {
    const plural = outcome.warnings.length === 1 ? '' : 's';
    console.log(
      `tkt_validate ok (${outcome.warnings.length} warning${plural})`,
    );
    return 0;
  }
  outcome.errors.forEach((error) => console.error(`error: ${error}`));
  const errorPlural = outcome.errors.length === 1 ? '' : 's';
  const warningPlural = outcome.warnings.length === 1 ? '' : 's';
  console.error(
    `tkt_validate failed (${outcome.errors.length} error${errorPlural}, ${outcome.warnings.length} warning${warningPlural})`,
  );
  return 1;
}

await runMain(main);
