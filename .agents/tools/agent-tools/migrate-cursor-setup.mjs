import path from 'node:path';
import { parseCli, runMain, runNx } from './lib/cli.mjs';
import {
  exists,
  isDirectory,
  listFilesRecursive,
  localDate,
  readText,
  relativePosix,
  resolveExisting,
  writeText,
} from './lib/fs-utils.mjs';

const MIGRATION_MARKER =
  '<!-- MIGRATED BY nx run agent-tools:migrate-cursor-setup -->';
const MEMORY_CATEGORIES = [
  'architecture',
  'decisions',
  'principles',
  'operations',
];
const SKILL_SOURCE_DIRS = [
  '.cursor/commands',
  '.cursor/skills',
  '.cursor/prompts',
  '.claude/commands',
  '.claude/skills',
];
const AGENT_SOURCE_DIRS = [
  '.cursor/agents',
  '.cursor/agent',
  '.claude/agents',
  '.claude/agent',
];
const MEMORY_SOURCE_DIRS = [
  '.cursor/rules',
  '.cursor/memory',
  '.cursor/memories',
  '.cursor/context',
  '.claude/rules',
  '.claude/memory',
  '.claude/memories',
  '.claude/context',
];
const SUPPORTED_EXTENSIONS = new Set(['.md', '.mdc', '.txt']);

function usage() {
  return `usage: migrate-cursor-setup.mjs [-h] --source SOURCE [--repo REPO]
                                [--dry-run] [--force] [--no-sync]

Migrate local Cursor/Claude setup into this repository's .agents layout.

options:
  -h, --help       show this help message and exit
  --source SOURCE  Full path to the existing repository
  --repo REPO      Target repository root (default: .)
  --dry-run        Plan and print a summary without writing files
  --force          Overwrite files not created by this migrator
  --no-sync        Do not run Nx agent-tool maintenance targets`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {
    source: { type: 'string', required: true },
    repo: { type: 'string', default: '.' },
    'dry-run': { type: 'boolean' },
    force: { type: 'boolean' },
    'no-sync': { type: 'boolean' },
  });
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const repo = path.resolve(expandHome(args.repo));
  const source = await resolveExisting(expandHome(args.source));
  if (!(await isDirectory(source))) {
    console.error(`error: source repo does not exist: ${source}`);
    return 1;
  }
  if (source === repo) {
    console.error('error: source and target repo must be different paths');
    return 1;
  }

  const report = createReport();
  const writes = await planMigration(repo, source, report, args.force);
  writes.push(...planReports(repo, source, writes, report));
  if (args['dry-run']) {
    console.log(renderConsoleSummary(writes, report, true));
    return 0;
  }
  await applyWrites(writes, report, args.force);
  if (!args['no-sync']) await runMaintenance(repo, source, report);
  console.log(renderConsoleSummary(writes, report, false));
  return report.commands.some((command) => command.returncode) ? 1 : 0;
}

function createReport() {
  return { migrated: [], skipped: [], ambiguous: [], commands: [] };
}

async function planMigration(repo, source, report, force) {
  const writes = [];
  const seen = new Set();
  for (const doc of await discoverDocs(source, SKILL_SOURCE_DIRS, seen)) {
    writes.push(...(await planSkill(repo, doc, force, report)));
  }
  for (const doc of await discoverDocs(source, AGENT_SOURCE_DIRS, seen)) {
    writes.push(...(await planAgent(repo, doc, force, report)));
  }
  for (const doc of await discoverDocs(source, MEMORY_SOURCE_DIRS, seen)) {
    writes.push(...(await planMemory(repo, doc, force, report)));
  }
  return writes;
}

async function discoverDocs(source, sourceDirs, seen) {
  const docs = [];
  for (const rawDir of sourceDirs) {
    const root = path.join(source, rawDir);
    for (const target of (await listFilesRecursive(root)).sort()) {
      if (!SUPPORTED_EXTENSIONS.has(path.extname(target).toLowerCase()))
        continue;
      const resolved = await resolveExisting(target);
      if (seen.has(resolved)) continue;
      seen.add(resolved);
      docs.push(await loadSourceDoc(source, target));
    }
  }
  return docs;
}

async function loadSourceDoc(source, target) {
  const content = await readText(target);
  const [frontmatter, bodyValue] = splitFrontmatter(content);
  const body = bodyValue.trim();
  const relPath = relativePosix(source, target);
  const title =
    extractTitle(body) ||
    frontmatter.name ||
    frontmatter.description ||
    path.basename(target, path.extname(target));
  const description = frontmatter.description || firstSentence(body) || title;
  return {
    path: target,
    relPath,
    stem: path.basename(target, path.extname(target)),
    frontmatter,
    body,
    title: title.trim(),
    description: description.trim(),
  };
}

function splitFrontmatter(content) {
  if (!content.startsWith('---\n')) return [{}, content];
  const end = content.indexOf('\n---', 4);
  if (end === -1) return [{}, content];
  const metadata = {};
  for (const rawLine of content.slice(4, end).split(/\r?\n/)) {
    const separator = rawLine.indexOf(':');
    if (separator === -1) continue;
    const key = rawLine.slice(0, separator).trim();
    const value = rawLine
      .slice(separator + 1)
      .trim()
      .replace(/^["']|["']$/g, '');
    if (key && value && !value.startsWith('[')) metadata[key] = value;
  }
  return [metadata, content.slice(end + 4).trimStart()];
}

async function planSkill(repo, doc, force, report) {
  const sourceSystem = detectSourceSystem(doc.relPath);
  const name = slugify(doc.frontmatter.name || doc.stem);
  const skillDir = path.join(repo, '.agents', 'skills', name);
  const description = compact(doc.description, 180);
  const body = doc.body || `# ${displayName(name)}\n\n${description}`;
  const skillContent = [
    '---',
    `name: ${name}`,
    `description: ${quoteYaml(description)}`,
    '---',
    '',
    MIGRATION_MARKER,
    `<!-- source: ${doc.relPath} -->`,
    `<!-- source_system: ${sourceSystem} -->`,
    '',
    normalizeHeading(body, displayName(name)),
    '',
  ].join('\n');
  const agentContent = [
    `# ${MIGRATION_MARKER}`,
    `# source: ${doc.relPath}`,
    `# source_system: ${sourceSystem}`,
    'interface:',
    `  display_name: ${quoteYaml(displayName(name))}`,
    `  short_description: ${quoteYaml(compact(description, 120))}`,
    `  default_prompt: ${quoteYaml(`Use $${name} in this repository.`)}`,
    'policy:',
    '  allow_implicit_invocation: false',
    '',
  ].join('\n');
  return filterExisting(
    [
      planned(
        path.join(skillDir, 'SKILL.md'),
        skillContent,
        'skill',
        doc.relPath,
      ),
      planned(
        path.join(skillDir, 'agents', 'openai.yaml'),
        agentContent,
        'skill-agent-metadata',
        doc.relPath,
      ),
    ],
    report,
    force,
  );
}

async function planAgent(repo, doc, force, report) {
  const sourceSystem = detectSourceSystem(doc.relPath);
  const id = slugify(doc.frontmatter.name || doc.stem);
  const today = localDate();
  const content = [
    '---',
    `id: ${id}`,
    `title: ${quoteYaml(displayName(id))}`,
    'type: agent',
    'status: active',
    `created: ${today}`,
    `updated: ${today}`,
    'tags:',
    '  - imported',
    `  - ${sourceSystem}`,
    '---',
    '',
    MIGRATION_MARKER,
    `<!-- source: ${doc.relPath} -->`,
    `<!-- source_system: ${sourceSystem} -->`,
    '',
    normalizeHeading(doc.body, displayName(id)),
    '',
  ].join('\n');
  return filterExisting(
    [
      planned(
        path.join(repo, '.agents', 'agents', `${id}.md`),
        content,
        'agent',
        doc.relPath,
      ),
    ],
    report,
    force,
  );
}

async function planMemory(repo, doc, force, report) {
  const sourceSystem = detectSourceSystem(doc.relPath);
  const [category, confidence, reason] = classifyMemory(doc);
  const id = slugify(doc.frontmatter.id || doc.stem);
  const today = localDate();
  const content = [
    '---',
    `id: ${id}`,
    `title: ${quoteYaml(doc.title)}`,
    `type: ${category}`,
    'status: active',
    `created: ${today}`,
    `updated: ${today}`,
    'tags:',
    '  - imported',
    `  - ${sourceSystem}`,
    `  - ${category}`,
    'read_when:',
    `  - reviewing imported ${sourceSystem} context from ${doc.relPath}`,
    '---',
    '',
    MIGRATION_MARKER,
    `<!-- source: ${doc.relPath} -->`,
    `<!-- source_system: ${sourceSystem} -->`,
    '',
    renderMemoryBody(doc, category, confidence, reason),
    '',
  ].join('\n');
  const write = planned(
    path.join(repo, '.agents', 'memory', category, `${id}.md`),
    content,
    'memory',
    doc.relPath,
    confidence,
    reason,
  );
  const filtered = await filterExisting([write], report, force);
  if (filtered.length && confidence !== 'high') report.ambiguous.push(write);
  return filtered;
}

function classifyMemory(doc) {
  const text =
    `${doc.relPath}\n${doc.title}\n${doc.description}\n${doc.body}`.toLowerCase();
  const scores = {
    decisions: countMatches(text, [
      'adr',
      'decision',
      'decided',
      'rationale',
      'tradeoff',
    ]),
    principles: countMatches(text, [
      'rule',
      'principle',
      'boundary',
      'must',
      'never',
      'always',
      'policy',
      'security',
    ]),
    operations: countMatches(text, [
      'command',
      'run ',
      'workflow',
      'deploy',
      'release',
      'maintenance',
      'debug',
      'incident',
    ]),
    architecture: countMatches(text, [
      'architecture',
      'structure',
      'routing',
      'component',
      'service',
      'system',
      'data flow',
    ]),
  };
  let category = MEMORY_CATEGORIES[0];
  for (const candidate of MEMORY_CATEGORIES.slice(1)) {
    if (scores[candidate] > scores[category]) category = candidate;
  }
  const best = scores[category];
  const tied = Object.keys(scores)
    .filter((name) => scores[name] === best)
    .sort();
  if (best === 0)
    return ['architecture', 'low', 'no strong category keywords found'];
  if (tied.length > 1)
    return [category, 'medium', `category tie: ${tied.join(', ')}`];
  return [category, 'high', ''];
}

function renderMemoryBody(doc, category, confidence, reason) {
  const lines = [
    `# ${doc.title}`,
    '',
    '## Digest',
    '',
    `- Category: \`${category}\``,
    `- Classification confidence: \`${confidence}\``,
  ];
  if (reason) lines.push(`- Review note: ${reason}`);
  lines.push(
    `- Source: \`${doc.relPath}\``,
    '',
    '## Durable Memory',
    '',
    compact(firstParagraph(doc.body) || doc.description || doc.title, 360),
    '',
    '## Imported Source',
    '',
    doc.body.trim() || doc.description,
  );
  return lines.join('\n');
}

function planReports(repo, source, writes, report) {
  const reportDir = path.join(repo, '.agents', 'migrations');
  const migrated = [...report.migrated, ...writes];
  return [
    planned(
      path.join(reportDir, 'cursor-migration-report.md'),
      renderReportMarkdown(repo, source, migrated, report),
      'migration-report',
      source,
    ),
    planned(
      path.join(reportDir, 'cursor-migration-manifest.json'),
      renderManifestJson(repo, source, migrated, report),
      'migration-manifest',
      source,
    ),
  ];
}

function renderReportMarkdown(repo, source, writes, report) {
  const lines = [
    '# Cursor/Claude Migration Report',
    '',
    MIGRATION_MARKER,
    '',
    `- Source: \`${source}\``,
    `- Target: \`${repo}\``,
    `- Generated: \`${localDate()}\``,
    `- Planned writes: \`${writes.length}\``,
    `- Skipped: \`${report.skipped.length}\``,
    `- Ambiguous: \`${report.ambiguous.length}\``,
    '',
    '## Migrated',
    '',
  ];
  if (writes.length) {
    [...writes]
      .sort((a, b) => a.destination.localeCompare(b.destination))
      .forEach((write) =>
        lines.push(
          `- \`${write.kind}\` \`${relativePosix(repo, write.destination)}\` from \`${write.source}\``,
        ),
      );
  } else lines.push('- None');
  lines.push('', '## Skipped', '');
  if (report.skipped.length) {
    report.skipped.forEach((item) =>
      lines.push(
        `- \`${item.destination}\` from \`${item.source}\`: ${item.reason}`,
      ),
    );
  } else lines.push('- None');
  lines.push('', '## Ambiguous', '');
  if (report.ambiguous.length) {
    report.ambiguous.forEach((write) =>
      lines.push(
        `- \`${relativePosix(repo, write.destination)}\` from \`${write.source}\`: ${write.ambiguousReason || 'review recommended'}`,
      ),
    );
  } else lines.push('- None');
  lines.push('', '## Maintenance Commands', '');
  if (report.commands.length) {
    report.commands.forEach((command) =>
      lines.push(
        `- \`${command.argv.join(' ')}\` -> \`${command.returncode}\``,
      ),
    );
  } else lines.push('- Not run yet');
  return `${lines.join('\n')}\n`;
}

function renderManifestJson(repo, source, writes, report) {
  const sortWrites = (items) =>
    [...items].sort((a, b) => a.destination.localeCompare(b.destination));
  return `${JSON.stringify(
    {
      ambiguous: report.ambiguous.map((write) => ({
        destination: relativePosix(repo, write.destination),
        reason: write.ambiguousReason,
        source: write.source,
      })),
      commands: report.commands.map((command) => ({
        argv: command.argv,
        returncode: command.returncode,
        stderr: command.stderr,
        stdout: command.stdout,
      })),
      generated: localDate(),
      migrated: sortWrites(writes).map((write) => ({
        ambiguous_reason: write.ambiguousReason,
        confidence: write.confidence,
        destination: relativePosix(repo, write.destination),
        kind: write.kind,
        source: write.source,
      })),
      migration_marker: MIGRATION_MARKER,
      skipped: report.skipped.map((item) => ({
        destination: item.destination,
        reason: item.reason,
        source: item.source,
      })),
      source,
      target: repo,
    },
    null,
    2,
  )}\n`;
}

async function filterExisting(writes, report, force) {
  const result = [];
  for (const write of writes) {
    if (
      (await exists(write.destination)) &&
      !force &&
      !(await isMigratedFile(write.destination))
    ) {
      report.skipped.push({
        destination: write.destination,
        source: write.source,
        reason:
          'destination exists and is not migrator-owned; rerun with --force to overwrite',
      });
    } else result.push(write);
  }
  return result;
}

async function applyWrites(writes, report, force) {
  for (const write of writes) {
    if (
      (await exists(write.destination)) &&
      !force &&
      !(await isMigratedFile(write.destination))
    ) {
      report.skipped.push({
        destination: write.destination,
        source: write.source,
        reason: 'destination appeared before write and is not migrator-owned',
      });
      continue;
    }
    await writeText(write.destination, write.content);
    report.migrated.push(write);
  }
}

async function runMaintenance(repo, source, report) {
  const targets = [
    ['sync-skills', []],
    ['tkt-sync', []],
    ['tkt-validate', ['.agents/tasks']],
  ];
  for (const [target, args] of targets) {
    const result = runNx(repo, target, args);
    report.commands.push({
      argv: [
        'nx',
        'run',
        `agent-tools:${target}`,
        ...(args.length ? ['--', ...args] : []),
      ],
      returncode: result.status,
      stdout: result.stdout.trim(),
      stderr: result.stderr.trim(),
    });
  }
  for (const write of planReports(repo, source, [], report)) {
    await writeText(write.destination, write.content);
  }
}

function planned(
  destination,
  content,
  kind,
  source,
  confidence = 'high',
  ambiguousReason = '',
) {
  return { destination, content, kind, source, confidence, ambiguousReason };
}

function renderConsoleSummary(writes, report, dryRun) {
  const failed = report.commands.filter((command) => command.returncode);
  const lines = [
    `cursor/claude migration ${dryRun ? 'planned' : 'migrated'}`,
    `writes: ${writes.length}`,
    `skipped: ${report.skipped.length}`,
    `ambiguous: ${report.ambiguous.length}`,
  ];
  if (failed.length) lines.push(`maintenance failures: ${failed.length}`);
  return lines.join('\n');
}

async function isMigratedFile(target) {
  try {
    const content = await readText(target);
    return (
      content.includes(MIGRATION_MARKER) ||
      content.includes('<!-- MIGRATED BY migrate-cursor-setup.mjs -->') ||
      content.includes('<!-- MIGRATED BY migrate_cursor_setup.py -->')
    );
  } catch {
    return false;
  }
}

function extractTitle(body) {
  const line = body
    .split(/\r?\n/)
    .find((value) => value.trim().startsWith('#'));
  return line ? line.trim().replace(/^#+/, '').trim() : '';
}

function firstSentence(body) {
  const paragraph = firstParagraph(body);
  return paragraph.match(/(.+?[.!?])(?:\s|$)/)?.[1] || paragraph;
}

function firstParagraph(body) {
  return (
    body
      .trim()
      .split(/\n\s*\n/)
      .map((part) => part.trim())
      .find((part) => part && !part.startsWith('#'))
      ?.split(/\s+/)
      .join(' ') || ''
  );
}

function normalizeHeading(body, fallbackTitle) {
  const clean = body.trim();
  if (!clean) return `# ${fallbackTitle}`;
  return clean.startsWith('#') ? clean : `# ${fallbackTitle}\n\n${clean}`;
}

function compact(value, limit) {
  const normalized = value.split(/\s+/).filter(Boolean).join(' ');
  return normalized.length <= limit
    ? normalized
    : `${normalized.slice(0, limit - 3).trimEnd()}...`;
}

function countMatches(text, needles) {
  return needles.reduce(
    (count, needle) => count + text.split(needle).length - 1,
    0,
  );
}

function slugify(value) {
  return (
    value
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/-{2,}/g, '-')
      .replace(/^-|-$/g, '') || 'imported'
  );
}

function displayName(slug) {
  return slug
    .split('-')
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join(' ');
}

function quoteYaml(value) {
  return `"${String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

function detectSourceSystem(relPath) {
  if (relPath.startsWith('.cursor/')) return 'cursor';
  if (relPath.startsWith('.claude/')) return 'claude';
  return 'unknown';
}

function expandHome(value) {
  if (value === '~') return process.env.HOME || value;
  if (value.startsWith('~/'))
    return path.join(process.env.HOME || '', value.slice(2));
  return value;
}

await runMain(main);
