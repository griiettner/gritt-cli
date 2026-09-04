import path from 'node:path';
import { parseCli, runMain, runProcess } from './lib/cli.mjs';
import { exists, isDirectory, readText } from './lib/fs-utils.mjs';
import { loadFrontmatter } from './frontmatter-utils.mjs';
import { resolveTicketIdentity } from './lib/tkt-identity.mjs';
import { findTicketDir, parseTicketRef } from './lib/tkt-store.mjs';

const TICKET_PATH_RE =
  /\.agents\/tasks\/(?:[A-Za-z0-9._-]+\/)?TKT-\d{4}-\d{4}\/TKT-\d{4}\//;
const REQUIRED_REPORT_SECTIONS = [
  '## Summary',
  '## Validation',
  '## Completion Gate',
];
const BENCHMARK_HINT_RE = /\bbenchmark|\bbench\b/i;

class Outcome {
  errors = [];
  warnings = [];
  notes = [];

  error(message) {
    this.errors.push(message);
  }
  warn(message) {
    this.warnings.push(message);
  }
  note(message) {
    this.notes.push(message);
  }
}

function usage() {
  return `usage: tkt-chain-check.mjs [-h] --ticket TICKET [--base BASE]
                           [--repo-root REPO_ROOT] [--require-report]
                           [--require-benchmark]

Run deterministic branch/ticket checks for tkt-exec-chain review.

options:
  -h, --help            show this help message and exit
  --ticket TICKET       Ticket id, for example TKT-0042 or login/TKT-0042.
  --base BASE           Base branch (default: main)
  --repo-root REPO_ROOT Project root containing .agents (default: .)
  --require-report      Treat missing report.md as an error
  --require-benchmark   Require explicit benchmark evidence`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {
    ticket: { type: 'string', required: true },
    base: { type: 'string', default: 'main' },
    'repo-root': { type: 'string', default: '.' },
    'require-report': { type: 'boolean' },
    'require-benchmark': { type: 'boolean' },
  });
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const outcome = new Outcome();
  const projectRoot = path.resolve(args['repo-root']);
  if (!parseTicketRef(args.ticket)) {
    console.error(`invalid ticket id: ${args.ticket}`);
    return 2;
  }
  const tasksRoot = path.join(projectRoot, '.agents', 'tasks');
  let preferredNamespace = null;
  try {
    preferredNamespace = (await resolveTicketIdentity(projectRoot, {
      persist: false,
    })).github_login;
  } catch {
    preferredNamespace = null;
  }
  let ticketDir;
  try {
    ticketDir = (
      await findTicketDir(tasksRoot, args.ticket, preferredNamespace)
    ).dir;
  } catch (error) {
    console.error(error.message);
    return 2;
  }
  await checkTicketArtifacts(ticketDir, outcome, args['require-report']);

  const gitRootValue = git(
    projectRoot,
    ['rev-parse', '--show-toplevel'],
    outcome,
  );
  if (!gitRootValue) return finish(outcome);
  const gitRoot = path.resolve(gitRootValue);
  outcome.note(`project root: ${projectRoot}`);
  outcome.note(`git root: ${gitRoot}`);

  const branch = git(gitRoot, ['rev-parse', '--abbrev-ref', 'HEAD'], outcome);
  const baseSha = git(gitRoot, ['rev-parse', args.base], outcome);
  const headSha = git(gitRoot, ['rev-parse', 'HEAD'], outcome);
  const mergeBase = git(gitRoot, ['merge-base', args.base, 'HEAD'], outcome);
  if (branch) {
    outcome.note(`current branch: ${branch}`);
    if (branch === args.base) {
      outcome.warn(
        `current branch is the base branch \`${args.base}\`; reviewer likely expects a worker branch`,
      );
    }
  }
  if (baseSha && headSha) {
    outcome.note(`base branch \`${args.base}\` sha: ${baseSha.slice(0, 12)}`);
    outcome.note(`head sha: ${headSha.slice(0, 12)}`);
  }
  if (mergeBase && baseSha && mergeBase !== baseSha) {
    outcome.warn(
      `HEAD is not based on the current tip of \`${args.base}\` (merge-base ${mergeBase.slice(0, 12)} != base ${baseSha.slice(0, 12)})`,
    );
  }
  const changedFiles = gitLines(
    gitRoot,
    ['diff', '--name-only', `${args.base}...HEAD`],
    outcome,
  );
  if (changedFiles) {
    outcome.note(
      `changed files against \`${args.base}\`: ${changedFiles.length}`,
    );
    changedFiles.forEach((file) => outcome.note(`  - ${file}`));
    checkChangedFiles(changedFiles, args.ticket, outcome);
  }
  if (args['require-benchmark'] || (await benchmarkExpected(ticketDir))) {
    await checkBenchmarkEvidence(ticketDir, outcome);
  }
  return finish(outcome);
}

async function checkTicketArtifacts(ticketDir, outcome, requireReport) {
  if (!(await isDirectory(ticketDir))) {
    outcome.error(`ticket folder does not exist: ${ticketDir}`);
    return;
  }
  const taskPath = path.join(ticketDir, 'task.md');
  const reportPath = path.join(ticketDir, 'report.md');
  if (!(await exists(taskPath))) {
    outcome.error(`missing task.md: ${taskPath}`);
  } else {
    const result = await loadFrontmatter(taskPath);
    result.errors.forEach((error) => outcome.error(error.render()));
    if (!Object.keys(result.metadata).length) {
      outcome.error(`task.md missing YAML frontmatter: ${taskPath}`);
    }
  }
  if (!(await exists(reportPath))) {
    const message = `missing report.md: ${reportPath}`;
    requireReport ? outcome.error(message) : outcome.warn(message);
    return;
  }
  const result = await loadFrontmatter(reportPath);
  result.errors.forEach((error) => outcome.error(error.render()));
  if (!Object.keys(result.metadata).length) {
    outcome.error(`report.md missing YAML frontmatter: ${reportPath}`);
  }
  let content;
  try {
    content = await readText(reportPath);
  } catch (error) {
    outcome.error(`cannot read report.md: ${error.message}`);
    return;
  }
  for (const section of REQUIRED_REPORT_SECTIONS) {
    if (!content.includes(section)) {
      outcome.warn(`report.md missing section \`${section}\``);
    }
  }
}

async function benchmarkExpected(ticketDir) {
  const taskPath = path.join(ticketDir, 'task.md');
  if (!(await exists(taskPath))) return false;
  try {
    return BENCHMARK_HINT_RE.test(await readText(taskPath));
  } catch {
    return false;
  }
}

async function checkBenchmarkEvidence(ticketDir, outcome) {
  const reportPath = path.join(ticketDir, 'report.md');
  if (!(await exists(reportPath))) {
    outcome.warn('benchmark expected but report.md is missing');
    return;
  }
  try {
    if (!BENCHMARK_HINT_RE.test(await readText(reportPath))) {
      outcome.warn(
        'benchmark expected but no benchmark evidence was found in report.md',
      );
    }
  } catch (error) {
    outcome.error(
      `cannot read report.md for benchmark check: ${error.message}`,
    );
  }
}

function checkChangedFiles(changedFiles, ticketId, outcome) {
  if (!changedFiles.length) {
    outcome.warn('no changed files detected against base branch');
    return;
  }
  const otherTickets = changedFiles.filter(
    (file) => TICKET_PATH_RE.test(file) && !file.includes(ticketId),
  );
  if (otherTickets.length) {
    outcome.warn('changed files include other ticket folders:');
    otherTickets.forEach((file) => outcome.warn(`  - ${file}`));
  }
  if (changedFiles.includes('.agents/tasks/backlog.yaml')) {
    outcome.note('backlog.yaml changed; verify this was intentional');
  }
}

function git(repoRoot, args, outcome) {
  const result = runProcess('git', args, { cwd: repoRoot });
  if (result.status !== 0) {
    const detail =
      result.stderr.trim() || result.stdout.trim() || `exit ${result.status}`;
    outcome.error(`git ${args.join(' ')} failed: ${detail}`);
    return null;
  }
  return result.stdout.trim();
}

function gitLines(repoRoot, args, outcome) {
  const output = git(repoRoot, args, outcome);
  if (output === null) return null;
  return output ? output.split(/\r?\n/).filter((line) => line.trim()) : [];
}

function finish(outcome) {
  outcome.notes.forEach((note) => console.log(`NOTE: ${note}`));
  outcome.warnings.forEach((warning) => console.log(`WARN: ${warning}`));
  outcome.errors.forEach((error) => console.error(`ERROR: ${error}`));
  if (outcome.errors.length) {
    console.error(
      `tkt_chain_check failed (${outcome.errors.length} error(s), ${outcome.warnings.length} warning(s))`,
    );
    return 1;
  }
  console.log(`tkt_chain_check ok (${outcome.warnings.length} warning(s))`);
  return 0;
}

await runMain(main);
