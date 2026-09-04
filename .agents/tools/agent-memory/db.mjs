import { readFile } from 'node:fs/promises';
import { mkdir } from 'node:fs/promises';
import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { createClient } from '@libsql/client';
import { agentsRoot, brainRoot, workspaceRoot } from './config.mjs';

export { agentsRoot, brainRoot, workspaceRoot };

const schema = await readFile(new URL('./schema.sql', import.meta.url), 'utf8');

function resolveDatabasePath() {
  return process.env.AGENT_MEMORY_TEST_DATABASE
    ? resolve(process.env.AGENT_MEMORY_TEST_DATABASE)
    : resolve(brainRoot, 'data', 'agent-memory.db');
}

let resolvedDatabasePath = resolveDatabasePath();
mkdirSync(dirname(resolvedDatabasePath), { recursive: true });

export let databasePath = resolvedDatabasePath;
export let database = createClient({
  url: `file:${resolvedDatabasePath}`,
});

function ensureDatabaseHandle() {
  const nextPath = resolveDatabasePath();
  if (nextPath === resolvedDatabasePath) {
    return;
  }

  resolvedDatabasePath = nextPath;
  databasePath = nextPath;
  mkdirSync(dirname(resolvedDatabasePath), { recursive: true });
  database = createClient({
    url: `file:${resolvedDatabasePath}`,
  });
}

async function tableHasColumn(table, column) {
  const info = await database.execute(`PRAGMA table_info(${table})`);
  return info.rows.some((row) => String(row.name) === column);
}

async function ensureMigrations() {
  if (!(await tableHasColumn('document_chunks', 'embedding'))) {
    await database.execute(
      'ALTER TABLE document_chunks ADD COLUMN embedding F32_BLOB(1536)',
    );
  }
}

export async function initializeDatabase() {
  ensureDatabaseHandle();
  await mkdir(dirname(resolvedDatabasePath), { recursive: true });
  await database.execute('PRAGMA journal_mode = WAL');

  const statements = schema
    .split(';')
    .map((statement) => statement.trim())
    .filter(Boolean);

  await database.batch(statements, 'write');
  await ensureMigrations();
}
