import path from 'node:path';
import { parseCli, runMain } from './lib/cli.mjs';
import { relativePosix } from './lib/fs-utils.mjs';
import {
  IDENTITY_RELATIVE_PATH,
  persistIdentity,
  resolveTicketIdentity,
} from './lib/tkt-identity.mjs';

function usage() {
  return `usage: tkt-identity.mjs [-h] [--repo-root REPO_ROOT] [--refresh]
                         [--namespace NAMESPACE] [--no-persist]

Resolve and store the GitHub login used as the local ticket namespace.

options:
  -h, --help              show this help message and exit
  --repo-root REPO_ROOT   Repository root (default: .)
  --refresh               Ignore the stored identity and query GitHub again
  --namespace NAMESPACE   Override the GitHub login
  --no-persist            Print the login without writing identity.local.yaml`;
}

async function main() {
  const args = parseCli(process.argv.slice(2), {
    'repo-root': { type: 'string', default: '.' },
    refresh: { type: 'boolean' },
    namespace: { type: 'string' },
    'no-persist': { type: 'boolean' },
  });
  if (args.help) {
    console.log(usage());
    return 0;
  }
  const repoRoot = path.resolve(args['repo-root']);
  const persist = !args['no-persist'];
  const identity = await resolveTicketIdentity(repoRoot, {
    namespace: args.namespace,
    refresh: args.refresh,
    persist,
  });
  if (persist) await persistIdentity(repoRoot, identity);
  console.log(identity.github_login);
  console.log(`source: ${identity.source}`);
  if (persist) {
    console.log(`stored: ${relativePosix(repoRoot, IDENTITY_RELATIVE_PATH)}`);
  }
  return 0;
}

await runMain(main);
