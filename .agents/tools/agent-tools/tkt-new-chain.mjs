import path from 'node:path';
import { CliError, parseCli, runAgentCli, runMain } from './lib/cli.mjs';
import {
  isDirectory,
  localDate,
  mkdir,
  relativePosix,
  writeText,
} from './lib/fs-utils.mjs';
import { resolveTicketIdentity } from './lib/tkt-identity.mjs';
import {
  nextTicketNumber,
  padTicketNumber,
  ticketDir,
} from './lib/tkt-store.mjs';

const TODO = 'TODO(tkt):';

function usage() {
  return `usage: tkt-new-chain.mjs [-h] --title TITLE --step SLUG:TITLE [SLUG:TITLE ...]
                         [--repo-root REPO_ROOT] [--namespace NAMESPACE]
                         [--owner OWNER] [--base-branch BASE_BRANCH]
                         [--branch-pattern BRANCH_PATTERN]
                         [--merge-policy MERGE_POLICY]
                         [--reviewer-title REVIEWER_TITLE] [--no-reviewer]
                         [--skills [SKILLS ...]] [--areas [AREAS ...]]
                         [--dependencies [DEPENDENCIES ...]]
                         [--create-concept] [--create-plan] [--no-sync]
                         [--dry-run]

Create a full PM/worker/reviewer ticket chain in the current developer's
GitHub-login namespace under .agents/tasks/.

The chain is always more than one ticket: one orchestrator, one worker ticket
per --step, and a final reviewer ticket unless --no-reviewer is passed. Use
gritt-agent ticket new when the work is a single one-shot ticket.

options:
  -h, --help            show this help message and exit
  --title TITLE         Orchestrator ticket title.
  --step SLUG:TITLE     Worker step, repeatable. At least two are required.
  --repo-root REPO_ROOT Repository root (default: .)
  --namespace NAMESPACE GitHub login override
  --owner OWNER         Owner frontmatter value (default: the namespace)
  --base-branch BRANCH  Chain base branch (default: main)
  --branch-pattern TEXT Branch naming pattern (default: tkt-{id}-{slug})
  --merge-policy TEXT   Merge policy text recorded in task.md
  --reviewer-title TEXT Final reviewer ticket title
  --no-reviewer         Do not create the final reviewer ticket
  --skills [ITEM ...]   Skills frontmatter list
  --areas [ITEM ...]    Areas frontmatter list
  --dependencies [...]  Orchestrator ticket dependencies
  --create-concept      Also create concept.md on the orchestrator
  --create-plan         Also create plan.md on the orchestrator
  --no-sync             Do not run the ticket index sync
  --dry-run             Show the planned chain without writing`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {
    title: { type: 'string', required: true },
    step: { type: 'multi', default: [] },
    'repo-root': { type: 'string', default: '.' },
    namespace: { type: 'string' },
    owner: { type: 'string' },
    'base-branch': { type: 'string', default: 'main' },
    'branch-pattern': { type: 'string', default: 'tkt-{id}-{slug}' },
    'merge-policy': {
      type: 'string',
      default:
        'Each worker opens a PR against main; reviewer runs after every PR; do not wait for CI/CD before merge when quota is unreliable.',
    },
    'reviewer-title': { type: 'string' },
    'no-reviewer': { type: 'boolean' },
    skills: { type: 'multi', default: ['tkt', 'tkt-exec-chain'] },
    areas: {
      type: 'multi',
      default: ['.agents/tasks', '.agents/skills', '.agents/tools'],
    },
    dependencies: { type: 'multi', default: [] },
    'create-concept': { type: 'boolean' },
    'create-plan': { type: 'boolean' },
    'no-sync': { type: 'boolean' },
    'dry-run': { type: 'boolean' },
  });
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const steps = parseSteps(args.step);
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
  const first = await nextTicketNumber(tasksRoot, namespace);
  const withReviewer = !args['no-reviewer'];
  const chain = buildChain({
    tasksRoot,
    namespace,
    first,
    title: args.title,
    steps,
    withReviewer,
    reviewerTitle: args['reviewer-title'],
  });

  if (args['dry-run']) {
    printChain(repoRoot, namespace, chain, args);
    if (!args['no-sync']) console.log('would run: gritt-agent ticket sync');
    return 0;
  }

  const today = localDate();
  const common = {
    namespace,
    owner: args.owner || namespace,
    created: today,
    updated: today,
    areas: args.areas,
    skills: args.skills,
    baseBranch: args['base-branch'],
    branchPattern: args['branch-pattern'],
    mergePolicy: args['merge-policy'],
  };

  await mkdir(chain.orchestrator.dir, { recursive: true });
  await writeText(
    path.join(chain.orchestrator.dir, 'task.md'),
    renderOrchestratorTask({ ...common, chain, dependencies: args.dependencies }),
  );
  if (args['create-concept']) {
    await writeText(
      path.join(chain.orchestrator.dir, 'concept.md'),
      renderConcept({ ...common, ticket: chain.orchestrator }),
    );
  }
  if (args['create-plan']) {
    await writeText(
      path.join(chain.orchestrator.dir, 'plan.md'),
      renderPlan({ ...common, chain }),
    );
  }
  for (const worker of chain.workers) {
    await mkdir(worker.dir, { recursive: true });
    await writeText(
      path.join(worker.dir, 'task.md'),
      renderWorkerTask({ ...common, chain, worker }),
    );
  }
  if (chain.reviewer) {
    await mkdir(chain.reviewer.dir, { recursive: true });
    await writeText(
      path.join(chain.reviewer.dir, 'task.md'),
      renderReviewerTask({ ...common, chain }),
    );
  }

  if (!args['no-sync']) {
    const result = runAgentCli(repoRoot, ['ticket', 'sync'], { inherit: true });
    if (result.status !== 0) return result.status;
  }
  printChain(repoRoot, namespace, chain, args);
  return 0;
}

function parseSteps(rawSteps) {
  if (rawSteps.length < 2) {
    throw new CliError(
      'a chain needs at least two --step values; use `gritt-agent ticket new` for a single one-shot ticket',
    );
  }
  return rawSteps.map((raw, index) => {
    const separator = raw.indexOf(':');
    const title = separator === -1 ? raw.trim() : raw.slice(separator + 1).trim();
    const slug =
      separator === -1 ? slugify(raw) : slugify(raw.slice(0, separator));
    if (!title) {
      throw new CliError(`--step ${raw} has no title`);
    }
    return { number: index + 1, slug, title };
  });
}

function buildChain(options) {
  const { tasksRoot, namespace, first, steps, withReviewer } = options;
  const make = (offset, title) => {
    const ticketId = `TKT-${padTicketNumber(first + offset)}`;
    return {
      ticketId,
      title,
      dir: ticketDir(tasksRoot, namespace, ticketId),
    };
  };
  const orchestrator = make(0, options.title);
  const workers = steps.map((step, index) => ({
    ...make(index + 1, step.title),
    step: step.number,
    slug: step.slug,
    total: steps.length,
  }));
  const reviewer = withReviewer
    ? make(
        steps.length + 1,
        options.reviewerTitle || `Review integrated ${options.title} chain`,
      )
    : null;
  return { orchestrator, workers, reviewer };
}

function printChain(repoRoot, namespace, chain, args) {
  console.log(chain.orchestrator.ticketId);
  console.log(`namespace: ${namespace}`);
  console.log(`qualified: ${namespace}/${chain.orchestrator.ticketId}`);
  console.log(relativePosix(repoRoot, chain.orchestrator.dir));
  console.log(
    relativePosix(repoRoot, path.join(chain.orchestrator.dir, 'task.md')),
  );
  if (args['create-concept']) {
    console.log(
      relativePosix(repoRoot, path.join(chain.orchestrator.dir, 'concept.md')),
    );
  }
  if (args['create-plan']) {
    console.log(
      relativePosix(repoRoot, path.join(chain.orchestrator.dir, 'plan.md')),
    );
  }
  chain.workers.forEach((worker) => {
    console.log(
      `worker ${worker.step}/${worker.total}: ${worker.ticketId} ${relativePosix(repoRoot, path.join(worker.dir, 'task.md'))}`,
    );
  });
  if (chain.reviewer) {
    console.log(
      `reviewer: ${chain.reviewer.ticketId} ${relativePosix(repoRoot, path.join(chain.reviewer.dir, 'task.md'))}`,
    );
  }
  console.log(
    `chain tickets: ${1 + chain.workers.length + (chain.reviewer ? 1 : 0)}`,
  );
  console.log(
    `${TODO} every scaffolded section must be replaced before execution; tkt-validate fails while any remains`,
  );
}

function slugify(value) {
  return (
    value
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '') || 'ticket'
  );
}

function ticketLink(fromDir, target) {
  const relative = path
    .relative(fromDir, path.join(target.dir, 'task.md'))
    .split(path.sep)
    .join('/');
  return `[${target.ticketId} ${target.title}](${relative})`;
}

function workerBranch(worker) {
  const number = worker.ticketId.toLowerCase().replace('tkt-', '');
  return `tkt-${number}-${padTicketNumber(worker.step).slice(-2)}-${worker.slug}`;
}

function renderOrchestratorTask(values) {
  const { chain } = values;
  const children = [
    ...chain.workers.map((worker) => worker.ticketId),
    ...(chain.reviewer ? [chain.reviewer.ticketId] : []),
  ];
  const listing = chain.workers.map(
    (worker, index) =>
      `${index + 1}. ${ticketLink(chain.orchestrator.dir, worker)}`,
  );
  if (chain.reviewer) {
    listing.push(
      `${chain.workers.length + 1}. ${ticketLink(chain.orchestrator.dir, chain.reviewer)} (final reviewer)`,
    );
  }
  return (
    frontmatter({
      ...values,
      ticketId: chain.orchestrator.ticketId,
      title: chain.orchestrator.title,
      artifact: 'task',
      status: 'planning',
      chainRole: 'orchestrator',
      chainChildren: children,
      dependencies: values.dependencies,
    }) +
    [
      `# ${chain.orchestrator.ticketId} Task: ${chain.orchestrator.title}`,
      '',
      '## Goal',
      '',
      `${TODO} state the concrete outcome this chain delivers.`,
      '',
      '## Chain Execution Contract',
      '',
      '- Execution mode: `tkt-exec-chain`',
      `- Base branch: \`${values.baseBranch}\``,
      `- Branch naming pattern: \`${values.branchPattern}\``,
      `- Worker branch pattern: \`tkt-{id}-{step}-{step-slug}\``,
      `- Merge policy: ${values.mergePolicy}`,
      '- Reviewer gate: reviewer runs after every worker PR',
      `- Child tickets: required and fixed as ${children[0]} through ${children[children.length - 1]}`,
      `- Validation required on every worker step: ${TODO} name the checks`,
      `- Benchmark requirements: ${TODO} name them or state none`,
      `- Final completion condition: ${TODO} state it`,
      '- Concurrency: exactly one active worker; no later step starts before the previous PR merges',
      '',
      '## Child Ticket Chain',
      '',
      ...listing,
      '',
      'The orchestrator activates exactly one worker ticket at a time. Every',
      'worker opens one PR and receives a reviewer verdict before merge. The next',
      'worker is activated only after that merge.',
      '',
      '## Inputs',
      '',
      `- ${TODO} list the plans, ADRs, and package READMEs a worker must read.`,
      '',
      '## Scope',
      '',
      `- ${TODO} describe the work covered by the child chain.`,
      '',
      '## Out of Scope',
      '',
      `- ${TODO} describe what the chain must not change.`,
      '',
      '## Acceptance Criteria',
      '',
      `- ${TODO} give concrete, checkable criteria.`,
      '',
      '## Verification',
      '',
      `- ${TODO} name the checks every worker and reviewer pass must respect.`,
      `- Run \`node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket ${chain.orchestrator.ticketId} --base ${values.baseBranch}\` before semantic review.`,
      '',
    ].join('\n')
  );
}

function renderWorkerTask(values) {
  const { chain, worker } = values;
  const index = chain.workers.indexOf(worker);
  const previous = index === 0 ? null : chain.workers[index - 1];
  return (
    frontmatter({
      ...values,
      ticketId: worker.ticketId,
      title: worker.title,
      artifact: 'task',
      status: index === 0 ? 'ready' : 'planning',
      chainRole: 'worker',
      chainParent: chain.orchestrator.ticketId,
      dependencies: previous ? [previous.ticketId] : [],
    }) +
    [
      `# ${worker.ticketId} Task: ${worker.title}`,
      '',
      '## Chain Role',
      '',
      `Worker ${worker.step} of ${worker.total} in the ${chain.orchestrator.ticketId} chain.`,
      previous
        ? `Start from a freshly updated \`${values.baseBranch}\` only after ${previous.ticketId} merges and passes review.`
        : `Start from a freshly updated \`${values.baseBranch}\`. This is the first worker in the chain.`,
      '',
      `Branch: \`${workerBranch(worker)}\``,
      '',
      '## Goal',
      '',
      `${TODO} state what this single step delivers.`,
      '',
      '## Scope',
      '',
      `- ${TODO} keep this to the one step; anything else belongs to another worker.`,
      '',
      '## Out of Scope',
      '',
      `- ${TODO} name the neighbouring steps this worker must not touch.`,
      '',
      '## Acceptance Criteria',
      '',
      `- ${TODO} give concrete criteria the reviewer can check on the PR.`,
      '',
      '## Verification',
      '',
      `- ${TODO} name the commands and manual checks for this step.`,
      `- Run \`node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket ${worker.ticketId} --base ${values.baseBranch}\` before semantic review.`,
      '',
      '## Handoff',
      '',
      'Report branch name, PR link, validation output, and unresolved risks to the',
      'PM, then stop. Do not start the next step.',
      '',
    ].join('\n')
  );
}

function renderReviewerTask(values) {
  const { chain } = values;
  return (
    frontmatter({
      ...values,
      ticketId: chain.reviewer.ticketId,
      title: chain.reviewer.title,
      artifact: 'task',
      status: 'planning',
      chainRole: 'reviewer',
      chainParent: chain.orchestrator.ticketId,
      dependencies: chain.workers.map((worker) => worker.ticketId),
    }) +
    [
      `# ${chain.reviewer.ticketId} Task: ${chain.reviewer.title}`,
      '',
      '## Chain Role',
      '',
      `Final reviewer ticket for the ${chain.orchestrator.ticketId} chain. Per-worker PR review stays`,
      'mandatory throughout the chain. This ticket runs the integrated pass after',
      `${chain.workers[chain.workers.length - 1].ticketId} and every earlier worker ticket have merged.`,
      '',
      '## Goal',
      '',
      'Independently determine whether the merged result satisfies the parent',
      'contract without scope drift, integration gaps, regressions, or missing',
      'evidence.',
      '',
      '## Review Scope',
      '',
      '- Re-run deterministic ticket and chain validation.',
      `- Review the full diff across ${chain.workers[0].ticketId} through ${chain.workers[chain.workers.length - 1].ticketId}.`,
      `- Load \`review/ticket\` against ${chain.orchestrator.ticketId}'s task.md for completion readiness, and \`review/impact\` across the merged diff for integration conflicts.`,
      `- ${TODO} name the architecture and behavior checks specific to this chain.`,
      '',
      '## Acceptance Criteria',
      '',
      '- Every parent and child acceptance criterion has evidence.',
      '- All worker PRs have recorded reviewer verdicts and required validation.',
      '- No unresolved high or medium finding blocks completion.',
      `- ${chain.orchestrator.ticketId} receives a completion report only after this reviewer returns \`pass\`.`,
      '',
      '## Verification',
      '',
      '- Run `gritt-agent ticket validate`.',
      `- Run \`node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket ${chain.reviewer.ticketId} --base ${values.baseBranch}\`.`,
      '- Re-run the scoped command set recorded by the parent and worker tickets.',
      '- Produce a typed verdict: `pass`, `needs-fix`, or `blocked`, with findings',
      '  and next actions.',
      '',
    ].join('\n')
  );
}

function renderConcept(values) {
  return (
    frontmatter({
      ...values,
      ticketId: values.ticket.ticketId,
      title: values.ticket.title,
      artifact: 'concept',
      status: 'concept',
      chainRole: 'orchestrator',
    }) +
    [
      `# ${values.ticket.ticketId} Concept: ${values.ticket.title}`,
      '',
      '## Problem',
      '',
      `${TODO} describe the user or product problem.`,
      '',
      '## Intent',
      '',
      `${TODO} describe what the chain is meant to achieve.`,
      '',
      '## Success Criteria',
      '',
      `- ${TODO} define what success looks like before execution starts.`,
      '',
    ].join('\n')
  );
}

function renderPlan(values) {
  const { chain } = values;
  const sequence = chain.workers.map(
    (worker) =>
      `${worker.step}. ${worker.ticketId} on \`${workerBranch(worker)}\`. ${TODO} describe the step.`,
  );
  if (chain.reviewer) {
    sequence.push(
      `${chain.workers.length + 1}. ${chain.reviewer.ticketId} runs the final integrated review.`,
    );
  }
  return (
    frontmatter({
      ...values,
      ticketId: chain.orchestrator.ticketId,
      title: chain.orchestrator.title,
      artifact: 'plan',
      status: 'planning',
      chainRole: 'orchestrator',
    }) +
    [
      `# ${chain.orchestrator.ticketId} Plan: ${chain.orchestrator.title}`,
      '',
      '## Sequence',
      '',
      ...sequence,
      '',
      'After each merge the reviewer runs the chain check, then a semantic pass.',
      '',
      '## Decisions To Lock Before Execution',
      '',
      `- ${TODO} record any open process or implementation decision, or state none.`,
      '',
    ].join('\n')
  );
}

function frontmatter(values) {
  const lines = [
    '---',
    `id: ${values.ticketId}`,
    `namespace: ${values.namespace}`,
    `title: ${values.title}`,
    `artifact: ${values.artifact}`,
    `status: ${values.status}`,
    `owner: ${values.owner}`,
    `created: ${values.created}`,
    `updated: ${values.updated}`,
  ];
  if (values.chainRole) lines.push(`chain_role: ${values.chainRole}`);
  if (values.chainParent) lines.push(`chain_parent: ${values.chainParent}`);
  appendList(lines, 'chain_children', values.chainChildren);
  appendList(lines, 'dependencies', values.dependencies);
  appendList(lines, 'areas', values.areas);
  appendList(lines, 'skills', values.skills);
  lines.push('---', '', '');
  return lines.join('\n');
}

function appendList(lines, name, values) {
  if (!values?.length) return;
  lines.push(`${name}:`);
  values.forEach((value) => lines.push(`  - ${value}`));
}

await runMain(main);
