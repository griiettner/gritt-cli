import path from 'node:path';
import { parseCli, runMain, runNx } from './lib/cli.mjs';
import {
  isDirectory,
  localDate,
  mkdir,
  removeDirectory,
  relativePosix,
  writeText,
} from './lib/fs-utils.mjs';
import { resolveTicketIdentity } from './lib/tkt-identity.mjs';
import {
  nextTicketNumber,
  padTicketNumber,
  ticketDir,
} from './lib/tkt-store.mjs';

function usage() {
  return `usage: tkt-new.mjs [-h] --title TITLE [--repo-root REPO_ROOT]
                   [--namespace NAMESPACE] [--owner OWNER]
                   [--create-concept] [--create-plan] [--no-sync]
                   [--dry-run]

Create a new ticket in the current developer's GitHub-login namespace.

options:
  -h, --help            show this help message and exit
  --title TITLE         Ticket title
  --repo-root REPO_ROOT Repository root (default: .)
  --namespace NAMESPACE GitHub login override
  --owner OWNER         Owner frontmatter value (default: the namespace)
  --create-concept      Also create concept.md
  --create-plan         Also create plan.md
  --no-sync             Do not run the ticket index sync
  --dry-run             Show the planned ticket without writing`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {
    title: { type: 'string', required: true },
    'repo-root': { type: 'string', default: '.' },
    namespace: { type: 'string' },
    owner: { type: 'string' },
    'create-concept': { type: 'boolean' },
    'create-plan': { type: 'boolean' },
    'no-sync': { type: 'boolean' },
    'dry-run': { type: 'boolean' },
  });
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const repoRoot = path.resolve(args['repo-root']);
  const tasksRoot = path.join(repoRoot, '.agents', 'tasks');
  if (!(await isDirectory(tasksRoot))) {
    console.error(`tasks root does not exist: ${tasksRoot}`);
    return 1;
  }
  const identity = await resolveTicketIdentity(repoRoot, {
    namespace: args.namespace,
  });
  const namespace = identity.github_login;
  const number = await nextTicketNumber(tasksRoot, namespace);
  const ticketId = `TKT-${padTicketNumber(number)}`;
  const owner = args.owner || namespace;
  const dir = ticketDir(tasksRoot, namespace, ticketId);
  const createdFiles = [path.join(dir, 'task.md')];
  if (args['create-concept']) createdFiles.push(path.join(dir, 'concept.md'));
  if (args['create-plan']) createdFiles.push(path.join(dir, 'plan.md'));
  if (args['dry-run']) {
    printTicket(repoRoot, namespace, ticketId, dir, createdFiles);
    if (!args['no-sync']) console.log('would run: nx run agent-tools:tkt-sync');
    return 0;
  }

  await mkdir(dir, { recursive: true });
  const today = localDate();
  const common = {
    ticketId,
    namespace,
    title: args.title,
    owner,
    created: today,
    updated: today,
  };
  await writeText(path.join(dir, 'task.md'), renderTask(common));
  if (args['create-concept']) {
    await writeText(path.join(dir, 'concept.md'), renderConcept(common));
  }
  if (args['create-plan']) {
    await writeText(path.join(dir, 'plan.md'), renderPlan(common));
  }
  if (!args['no-sync']) {
    const result = runNx(repoRoot, 'tkt-sync', [], { inherit: true });
    if (result.status !== 0) {
      await removeDirectory(dir);
      console.error(
        `ticket creation rolled back because index sync failed for ${ticketId}; no ticket number was consumed`,
      );
      return result.status;
    }
  }
  printTicket(repoRoot, namespace, ticketId, dir, createdFiles);
  return 0;
}

function printTicket(repoRoot, namespace, ticketId, dir, createdFiles) {
  console.log(ticketId);
  console.log(`namespace: ${namespace}`);
  console.log(`qualified: ${namespace}/${ticketId}`);
  console.log(relativePosix(repoRoot, dir));
  createdFiles.forEach((file) => console.log(relativePosix(repoRoot, file)));
}

function renderTask(values) {
  return (
    frontmatter({ ...values, artifact: 'task', status: 'ready' }) +
    [
      `# ${values.ticketId} Task: ${values.title}`,
      '',
      '## Goal',
      '',
      'Define the concrete execution goal here.',
      '',
      '## Inputs',
      '',
      '- Add the required references here.',
      '',
      '## Scope',
      '',
      '- Define the exact work this ticket may change.',
      '',
      '## Out of Scope',
      '',
      '- Define what this ticket must not change.',
      '',
      '## Acceptance Criteria',
      '',
      '- Define concrete acceptance criteria.',
      '',
      '## Verification',
      '',
      '- Define the checks that prove the work is done.',
      '',
    ].join('\n')
  );
}

function renderConcept(values) {
  return (
    frontmatter({ ...values, artifact: 'concept', status: 'concept' }) +
    [
      `# ${values.ticketId} Concept: ${values.title}`,
      '',
      '## Problem',
      '',
      'Describe the user or product problem here.',
      '',
      '## Intent',
      '',
      'Describe what the ticket is meant to achieve.',
      '',
      '## Success Criteria',
      '',
      '- Define what success looks like before execution starts.',
      '',
    ].join('\n')
  );
}

function renderPlan(values) {
  return (
    frontmatter({ ...values, artifact: 'plan', status: 'planning' }) +
    [
      `# ${values.ticketId} Plan: ${values.title}`,
      '',
      '## Sequence',
      '',
      '1. Lock remaining product or implementation decisions.',
      '2. Execute the scoped change.',
      '3. Verify against the ticket acceptance criteria.',
      '',
      '## Decisions To Lock Before Execution',
      '',
      '- Fill in any still-open process or implementation decisions here.',
      '',
    ].join('\n')
  );
}

function frontmatter(values) {
  return [
    '---',
    `id: ${values.ticketId}`,
    `namespace: ${values.namespace}`,
    `title: ${values.title}`,
    `artifact: ${values.artifact}`,
    `status: ${values.status}`,
    `owner: ${values.owner}`,
    `created: ${values.created}`,
    `updated: ${values.updated}`,
    '---',
    '',
    '',
  ].join('\n');
}

await runMain(main);
