import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const toolsRoot = path.dirname(fileURLToPath(import.meta.url));

function run(tool, args = [], options = {}) {
  const result = spawnSync(
    process.execPath,
    [path.join(toolsRoot, tool), ...args],
    {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      encoding: 'utf8',
    },
  );
  assert.equal(
    result.status,
    options.status ?? 0,
    `${tool} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

async function temporaryDirectory(name) {
  return mkdtemp(path.join(os.tmpdir(), `gritt--`));
}

async function write(target, content) {
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, content, 'utf8');
}

test('skill and chain ticket scaffolds write expected files', async () => {
  const repo = await temporaryDirectory('scaffolds');
  await mkdir(path.join(repo, '.agents/tasks'), { recursive: true });

  run('create-skill.mjs', [
    'sample-skill',
    'Sample description.',
    '--repo',
    repo,
    '--no-sync',
  ]);
  run('tkt-new-chain.mjs', [
    '--title',
    'Sample chain',
    '--repo-root',
    repo,
    '--namespace',
    'test-user',
    '--create-concept',
    '--create-plan',
    '--no-sync',
    '--step',
    'one:First step',
    '--step',
    'two:Second step',
  ]);

  assert.match(
    await readFile(
      path.join(repo, '.agents/skills/sample-skill/SKILL.md'),
      'utf8',
    ),
    /name: sample-skill/,
  );
  const chainTask = (id) =>
    readFile(
      path.join(repo, `.agents/tasks/test-user/TKT-0001-0025/${id}/task.md`),
      'utf8',
    );
  const task = await chainTask('TKT-0001');
  assert.match(task, /namespace: test-user/);
  assert.match(task, /Execution mode: `tkt-exec-chain`/);
  assert.match(task, /tkt-chain-check\.mjs --ticket/);

  // A chain is an orchestrator, one ticket per step, and a final reviewer.
  assert.match(task, /chain_role: orchestrator/);
  assert.match(task, /chain_children:\n {2}- TKT-0002\n {2}- TKT-0003\n {2}- TKT-0004/);
  assert.match(task, /## Child Ticket Chain/);

  const firstWorker = await chainTask('TKT-0002');
  assert.match(firstWorker, /chain_role: worker/);
  assert.match(firstWorker, /chain_parent: TKT-0001/);
  assert.match(firstWorker, /Worker 1 of 2/);
  assert.match(firstWorker, /Branch: `tkt-0002-01-one`/);
  assert.doesNotMatch(firstWorker, /dependencies:/);

  const secondWorker = await chainTask('TKT-0003');
  assert.match(secondWorker, /Worker 2 of 2/);
  assert.match(secondWorker, /dependencies:\n {2}- TKT-0002/);

  const reviewer = await chainTask('TKT-0004');
  assert.match(reviewer, /chain_role: reviewer/);
  assert.match(reviewer, /chain_parent: TKT-0001/);
});

test('a chain scaffold refuses to collapse into a single ticket', async () => {
  const repo = await temporaryDirectory('chain-guard');
  await mkdir(path.join(repo, '.agents/tasks'), { recursive: true });

  const noSteps = run(
    'tkt-new-chain.mjs',
    ['--title', 'Lonely chain', '--repo-root', repo, '--no-sync'],
    { status: 2 },
  );
  assert.match(noSteps.stderr, /at least two --step values/);

  const oneStep = run(
    'tkt-new-chain.mjs',
    [
      '--title',
      'Lonely chain',
      '--repo-root',
      repo,
      '--no-sync',
      '--step',
      'only:Only step',
    ],
    { status: 2 },
  );
  assert.match(oneStep.stderr, /at least two --step values/);
});

test('migration dry-run and execution work without Python', async () => {
  const root = await temporaryDirectory('migration');
  const source = path.join(root, 'source');
  const target = path.join(root, 'target');
  await write(
    path.join(source, '.cursor/skills/example.md'),
    `---
name: example
description: Example imported skill.
---

# Example
`,
  );
  await mkdir(path.join(target, '.agents/tasks'), { recursive: true });

  run('migrate-cursor-setup.mjs', [
    '--source',
    source,
    '--repo',
    target,
    '--dry-run',
    '--no-sync',
  ]);
  run('migrate-cursor-setup.mjs', [
    '--source',
    source,
    '--repo',
    target,
    '--no-sync',
  ]);

  const migrated = await readFile(
    path.join(target, '.agents/skills/example/SKILL.md'),
    'utf8',
  );
  assert.match(migrated, /MIGRATED BY nx run agent-tools:migrate-cursor-setup/);
});

test('Codex trust uses an isolated configuration directory', async () => {
  const root = await temporaryDirectory('trust');
  const repo = path.join(root, 'repo');
  const codexHome = path.join(root, 'codex');
  await mkdir(repo, { recursive: true });

  run('trust-codex-project.mjs', [repo], { env: { CODEX_HOME: codexHome } });
  run('trust-codex-project.mjs', ['--check', repo], {
    env: { CODEX_HOME: codexHome },
  });

  const config = await readFile(path.join(codexHome, 'config.toml'), 'utf8');
  assert.match(config, /trust_level = "trusted"/);
});

test('all command entry points provide help', () => {
  const commands = [
    'create-skill.mjs',
    'migrate-cursor-setup.mjs',
    'tkt-chain-check.mjs',
    'tkt-identity.mjs',
    'tkt-new-chain.mjs',
    'trust-codex-project.mjs',
  ];
  for (const command of commands) {
    assert.match(run(command, ['--help']).stdout, /usage:/);
  }
});
