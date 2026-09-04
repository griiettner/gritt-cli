import path from 'node:path';
import { CliError, parseCli, runAgentCli, runMain } from './lib/cli.mjs';
import { exists, relativePosix, writeText } from './lib/fs-utils.mjs';

const MAX_NAME_LENGTH = 64;

function usage() {
  return `usage: create-skill.mjs [-h] [--title TITLE] [--repo REPO] [--force]
                        [--no-openai] [--no-sync] [--dry-run]
                        name description

Create a project-local agent skill.

positional arguments:
  name          Skill name; normalized to lowercase kebab-case
  description   Skill discovery description

options:
  -h, --help     show this help message and exit
  --title TITLE  Human-readable heading/display name
  --repo REPO    Repository root
  --force        Overwrite an existing skill
  --no-openai    Do not create agents/openai.yaml
  --no-sync      Do not refresh generated adapters
  --dry-run      Validate and show planned files without writing`;
}

async function main() {
  const args = parseCli(
    process.argv.slice(2),
    {
      title: { type: 'string' },
      repo: { type: 'string', default: '.' },
      force: { type: 'boolean' },
      'no-openai': { type: 'boolean' },
      'no-sync': { type: 'boolean' },
      'dry-run': { type: 'boolean' },
    },
    [{ name: 'name' }, { name: 'description' }],
  );
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const repo = path.resolve(args.repo);
  const skillName = normalizeName(args.name);
  validateName(skillName);
  const skillFile = path.join(repo, '.agents', 'skills', skillName, 'SKILL.md');
  const agentFile = path.join(
    repo,
    '.agents',
    'skills',
    skillName,
    'agents',
    'openai.yaml',
  );
  if ((await exists(skillFile)) && !args.force) {
    throw new CliError(`skill already exists: ${skillFile}`, 1);
  }
  const title = args.title || displayName(skillName);
  if (args['dry-run']) {
    console.log(`would create skill: ${relativePosix(repo, skillFile)}`);
    if (!args['no-openai']) {
      console.log(
        `would create Codex metadata: ${relativePosix(repo, agentFile)}`,
      );
    }
    if (!args['no-sync']) {
      console.log('would run: gritt-agent skill sync');
    }
    return 0;
  }

  await writeText(skillFile, renderSkill(skillName, args.description, title));
  if (!args['no-openai']) {
    await writeText(
      agentFile,
      renderOpenaiYaml(skillName, args.description, title),
    );
  }
  if (!args['no-sync']) {
    const result = runAgentCli(repo, ['skill', 'sync'], { inherit: true });
    if (result.status !== 0) return result.status;
  }
  console.log(`created skill: .agents/skills/${skillName}/SKILL.md`);
  if (!args['no-openai']) {
    console.log(
      `created Codex metadata: .agents/skills/${skillName}/agents/openai.yaml`,
    );
  }
  if (!args['no-sync']) console.log('synced Claude stubs');
  return 0;
}

function normalizeName(rawName) {
  return rawName
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/-{2,}/g, '-')
    .replace(/^-|-$/g, '');
}

function validateName(name) {
  if (!name) {
    throw new CliError(
      'skill name must contain at least one letter or digit',
      1,
    );
  }
  if (name.length > MAX_NAME_LENGTH) {
    throw new CliError(
      `skill name must be ${MAX_NAME_LENGTH} characters or fewer`,
      1,
    );
  }
}

function renderSkill(name, description, title) {
  return [
    '---',
    `name: ${name}`,
    `description: ${quoteYaml(description)}`,
    'disable-model-invocation: true',
    '---',
    '',
    `# ${title}`,
    '',
    '## Purpose',
    '',
    description,
    '',
    '## Workflow',
    '',
    '1. Read `AGENTS.md`.',
    '2. Gather only the context needed for the requested work.',
    "3. Follow this skill's workflow and keep changes scoped.",
    '4. Run relevant validation before reporting completion.',
    '',
    '## Output',
    '',
    '- State what changed.',
    '- Report validation performed.',
    '- Call out unresolved follow-up or risk.',
    '',
  ].join('\n');
}

function renderOpenaiYaml(name, description, title) {
  return [
    'interface:',
    `  display_name: ${quoteYaml(title)}`,
    `  short_description: ${quoteYaml(shortDescription(description))}`,
    `  default_prompt: ${quoteYaml(`Use $${name} in this repository.`)}`,
    'policy:',
    '  allow_implicit_invocation: false',
    '',
  ].join('\n');
}

function displayName(name) {
  return name
    .split('-')
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join(' ');
}

function shortDescription(description) {
  const normalized = description.split(/\s+/).filter(Boolean).join(' ');
  if (normalized.length <= 120) return normalized;
  const sentence = normalized.match(/^.*?[.!?](?:\s|$)/)?.[0]?.trim();
  if (sentence && sentence.length <= 120) return sentence;
  let clipped = normalized.slice(0, 117);
  if (clipped.includes(' '))
    clipped = clipped.slice(0, clipped.lastIndexOf(' '));
  return `${clipped.replace(/[ ,;:]+$/, '')}...`;
}

function quoteYaml(value) {
  return `"${String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

await runMain(main);
