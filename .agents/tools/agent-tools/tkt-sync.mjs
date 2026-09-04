import path from 'node:path';
import { parseCli, runMain } from './lib/cli.mjs';
import {
  exists,
  isDirectory,
  listDirectories,
  listEntries,
  readText,
  relativePosix,
  removeFile,
  writeText,
} from './lib/fs-utils.mjs';
import { loadFrontmatter } from './frontmatter-utils.mjs';
import {
  SHARED_NAMESPACE,
  TASK_SHARD_SIZE,
  chunkName,
  isChunkDirName,
  iterTicketDirs,
  listNamespaces,
  padTicketNumber,
  ticketNumber,
} from './lib/tkt-store.mjs';

const ARTIFACTS = ['concept', 'plan', 'task', 'report'];

function usage() {
  return `usage: tkt-sync.mjs [-h] [--check] [repo]

Regenerate helper indexes for agent tickets and memory.

positional arguments:
  repo        Repository root (default: .)

options:
  -h, --help  show this help message and exit
  --check     Report generated-index drift without writing`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), { check: { type: 'boolean' } }, [
    { name: 'repo', required: false, default: '.' },
  ]);
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const repo = path.resolve(args.repo);
  const tasksRoot = path.join(repo, '.agents', 'tasks');
  const memoryRoot = path.join(repo, '.agents', 'memory');
  if (!(await isDirectory(tasksRoot))) {
    console.error(`error: missing tasks root: ${tasksRoot}`);
    return 1;
  }

  const errors = [];
  const drift = [];
  await syncTasksIndex(repo, tasksRoot, errors, drift, args.check);
  await syncMemoryIndexes(repo, memoryRoot, errors, drift, args.check);
  if (errors.length) {
    for (const error of errors) console.error(`error: ${error.render()}`);
    console.error(`tkt_sync failed (${errors.length} frontmatter error(s))`);
    return 1;
  }
  if (args.check) {
    if (drift.length) {
      for (const item of drift) console.error(`drift: ${item}`);
      console.error(
        `tkt_sync: ${drift.length} generated index file(s) out of sync`,
      );
      return 1;
    }
    console.log('tkt_sync ok (no drift)');
    return 0;
  }
  console.log('synced .agents task and memory indexes');
  return 0;
}

async function syncTasksIndex(repo, tasksRoot, errors, drift, check) {
  const entries = [];
  for (const ticket of await iterTicketDirs(tasksRoot)) {
    const artifacts = await existingArtifacts(repo, ticket.dir);
    const primary = await firstExisting(ticket.dir, [
      'task',
      'concept',
      'plan',
      'report',
    ]);
    const metadata = primary ? await readFrontmatter(primary, errors) : {};
    const reportMetadata = await readFrontmatter(
      path.join(ticket.dir, 'report.md'),
      errors,
    );
    const created = metadata.created || '';
    entries.push({
      id: ticket.ticketId,
      namespace: ticket.namespace,
      title: metadata.title || ticket.ticketId,
      status: reportMetadata.status || metadata.status || 'planning',
      owner: metadata.owner || ticket.namespace,
      priority: 'normal',
      created,
      updated: (await newestUpdated(ticket.dir)) || metadata.updated || created,
      artifacts,
      dependencies: metadata.dependencies || [],
      areas: metadata.areas || [],
      skills: metadata.skills || [],
    });
  }
  await writeTaskIndexes(repo, tasksRoot, entries, drift, check);
}

async function writeTaskIndexes(repo, tasksRoot, entries, drift, check) {
  const shards = [];
  const known = new Set(
    (await listNamespaces(tasksRoot)).map((namespace) => namespace.id),
  );
  for (const entry of entries) known.add(entry.namespace);

  for (const namespace of [...known].sort()) {
    const namespaceEntries = entries.filter(
      (entry) => entry.namespace === namespace,
    );
    const namespaceRoot =
      namespace === SHARED_NAMESPACE
        ? tasksRoot
        : path.join(tasksRoot, namespace);
    const maximum = namespaceEntries.reduce(
      (value, entry) => Math.max(value, ticketNumber(entry.id)),
      0,
    );
    const expectedChunks = new Set();
    for (let start = 1; start <= maximum; start += TASK_SHARD_SIZE) {
      const end = start + TASK_SHARD_SIZE - 1;
      const shardEntries = namespaceEntries.filter((entry) => {
        const number = ticketNumber(entry.id);
        return start <= number && number <= end;
      });
      if (!shardEntries.length) continue;
      const name = chunkName(start);
      expectedChunks.add(name);
      const shardPath = path.join(namespaceRoot, name, 'index.yaml');
      await updateGenerated(
        repo,
        shardPath,
        renderTasksIndex(shardEntries),
        drift,
        check,
      );
      shards.push(
        buildShardMetadata(
          repo,
          namespace,
          start,
          end,
          shardPath,
          shardEntries,
        ),
      );
    }

    if (await isDirectory(namespaceRoot)) {
      for (const chunk of await listDirectories(namespaceRoot)) {
        if (!isChunkDirName(chunk.name) || expectedChunks.has(chunk.name)) {
          continue;
        }
        const shardPath = path.join(namespaceRoot, chunk.name, 'index.yaml');
        if (!(await exists(shardPath))) continue;
        drift.push(`${relativePosix(repo, shardPath)} (stale)`);
        if (!check) await removeFile(shardPath);
      }
    }
  }

  await updateGenerated(
    repo,
    path.join(tasksRoot, 'index.yaml'),
    renderTaskRouter(shards),
    drift,
    check,
  );
}

async function syncMemoryIndexes(repo, memoryRoot, errors, drift, check) {
  if (!(await isDirectory(memoryRoot))) return;
  for (const category of await listDirectories(memoryRoot)) {
    const categoryDir = path.join(memoryRoot, category.name);
    const memories = [];
    for (const entry of await listEntries(categoryDir)) {
      if (
        !entry.isFile() ||
        !entry.name.endsWith('.md') ||
        entry.name === 'index.md'
      ) {
        continue;
      }
      const memoryPath = path.join(categoryDir, entry.name);
      const metadata = await readFrontmatter(memoryPath, errors);
      const stem = entry.name.slice(0, -3);
      memories.push({
        id: metadata.id || stem,
        title: metadata.title || titleFromSlug(stem),
        file: entry.name,
        tags: metadata.tags || [],
        read_when: metadata.read_when || [],
      });
    }
    if (memories.length) {
      await updateGenerated(
        repo,
        path.join(categoryDir, 'index.yaml'),
        renderMemoryIndex(memories),
        drift,
        check,
      );
    }
  }
}

async function updateGenerated(repo, target, desired, drift, check) {
  const current = await readText(target, '');
  if (current === desired) return;
  drift.push(relativePosix(repo, target));
  if (!check) await writeText(target, desired);
}

async function existingArtifacts(repo, ticketDir) {
  const artifacts = {};
  for (const artifact of ARTIFACTS) {
    const target = path.join(ticketDir, `${artifact}.md`);
    if (await exists(target)) artifacts[artifact] = relativePosix(repo, target);
  }
  const updates = path.join(ticketDir, 'updates');
  if (await isDirectory(updates))
    artifacts.updates = `${relativePosix(repo, updates)}/`;
  return artifacts;
}

async function firstExisting(ticketDir, names) {
  for (const name of names) {
    const target = path.join(ticketDir, `${name}.md`);
    if (await exists(target)) return target;
  }
  return null;
}

async function newestUpdated(ticketDir) {
  const targets = [];
  for (const entry of await listEntries(ticketDir)) {
    if (entry.isFile() && entry.name.endsWith('.md')) {
      targets.push(path.join(ticketDir, entry.name));
    }
  }
  const updates = path.join(ticketDir, 'updates');
  if (await isDirectory(updates)) {
    for (const entry of await listEntries(updates)) {
      if (entry.isFile() && entry.name.endsWith('.md')) {
        targets.push(path.join(updates, entry.name));
      }
    }
  }
  const values = [];
  for (const target of targets.sort()) {
    const { metadata, errors } = await loadFrontmatter(target);
    if (!errors.length && metadata.updated)
      values.push(String(metadata.updated));
  }
  return values.sort().at(-1) || '';
}

async function readFrontmatter(target, errors) {
  const result = await loadFrontmatter(target);
  errors.push(...result.errors);
  return result.metadata;
}

function renderTasksIndex(entries) {
  const lines = ['tickets:'];
  for (const entry of entries) {
    lines.push(
      `  - id: ${entry.id}`,
      `    namespace: ${entry.namespace}`,
      `    title: ${entry.title}`,
      `    status: ${entry.status}`,
      `    owner: ${entry.owner}`,
      `    priority: ${entry.priority}`,
      `    created: ${entry.created}`,
      `    updated: ${entry.updated}`,
      '    artifacts:',
    );
    for (const [key, value] of Object.entries(entry.artifacts)) {
      lines.push(`      ${key}: ${value}`);
    }
    renderList(lines, 'dependencies', entry.dependencies, '    ');
    renderList(lines, 'areas', entry.areas, '    ');
    renderList(lines, 'skills', entry.skills, '    ');
  }
  return `${lines.join('\n')}\n`;
}

function renderTaskRouter(shards) {
  if (!shards.length) {
    return [
      '# Generated chunk router for .agents/tasks/.',
      '# No tickets yet. Create one with the tkt-new skill, then rerun:',
      '#   nx run agent-tools:tkt-sync',
      '',
    ].join('\n');
  }
  const lines = [];
  for (const shard of shards) {
    lines.push(
      `- namespace: ${shard.namespace}`,
      `  range: ${shard.range}`,
      `  file: ${shard.file}`,
      `  count: ${shard.count}`,
      `  updated: ${shard.updated}`,
      '  statuses:',
    );
    for (const [status, count] of Object.entries(shard.statuses)) {
      lines.push(`    - ${status}: ${count}`);
    }
    renderList(lines, 'areas', shard.areas, '  ');
  }
  return `${lines.join('\n')}\n`;
}

function renderMemoryIndex(memories) {
  const lines = ['memories:'];
  for (const memory of memories) {
    lines.push(
      `  - id: ${memory.id}`,
      `    title: ${memory.title}`,
      `    file: ${memory.file}`,
    );
    renderList(lines, 'tags', memory.tags, '    ');
    renderList(lines, 'read_when', memory.read_when, '    ');
  }
  return `${lines.join('\n')}\n`;
}

function renderList(lines, key, values, indent) {
  if (Array.isArray(values) && values.length) {
    lines.push(`${indent}${key}:`);
    for (const value of values) lines.push(`${indent}  - ${value}`);
  } else {
    lines.push(`${indent}${key}: []`);
  }
}

function buildShardMetadata(repo, namespace, start, end, shardPath, entries) {
  const statuses = {};
  const areas = new Set();
  const updated = [];
  for (const entry of entries) {
    statuses[entry.status] = (statuses[entry.status] || 0) + 1;
    if (Array.isArray(entry.areas))
      entry.areas.forEach((area) => areas.add(String(area)));
    if (entry.updated) updated.push(String(entry.updated));
  }
  return {
    namespace,
    range: `TKT-${padTicketNumber(start)}-${padTicketNumber(end)}`,
    file: relativePosix(repo, shardPath),
    count: entries.length,
    updated: updated.sort().at(-1) || '',
    statuses: Object.fromEntries(
      Object.entries(statuses).sort(([left], [right]) =>
        left.localeCompare(right),
      ),
    ),
    areas: [...areas].sort(),
  };
}

function titleFromSlug(slug) {
  return slug
    .replace(/[-_]/g, ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

await runMain(main);
