import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

test('searchKnowledge returns FTS5 hits when endpoint calls fail', async (t) => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'gritt-memory-search-'));
  const databaseFile = path.join(directory, 'agent-memory.db');

  process.env.AGENT_MEMORY_TEST_DATABASE = databaseFile;
  process.env.AGENT_MEMORY_API_KEY = 'sk-test';
  process.env.AGENT_MEMORY_BASE_URL = 'https://example.test';
  process.env.AGENT_EMBEDDING_PROVIDER = 'text-embedding-3-small';
  process.env.AGENT_RERANK_PROVIDER = 'rerank-3.5';

  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error('network down');
  };
  t.after(() => {
    globalThis.fetch = originalFetch;
    delete process.env.AGENT_MEMORY_TEST_DATABASE;
  });

  const stamp = Date.now();
  const { initializeDatabase, database } = await import(`./db.mjs?fallback=${stamp}`);
  const { searchKnowledge } = await import(`./search.mjs?fallback=${stamp}`);

  await initializeDatabase();
  await database.execute({
    sql: `
      INSERT INTO documents(path, title, content, content_hash, source_mtime)
      VALUES (?, ?, ?, ?, ?)
    `,
    args: [
      'docs/alpha.md',
      'alpha',
      'FTS fallback memory content',
      'hash-a',
      Date.now(),
    ],
  });
  await database.execute({
    sql: `
      INSERT INTO document_chunks(
        document_id, chunk_index, heading, start_line, end_line, content, content_hash
      ) VALUES (1, 0, 'Intro', 1, 1, 'FTS fallback memory content', 'hash-c')
    `,
  });
  await database.execute(
    "INSERT INTO document_chunks_fts(document_chunks_fts) VALUES ('rebuild')",
  );

  const rows = await searchKnowledge('FTS fallback', 5);
  assert.equal(rows.length, 1);
  assert.match(rows[0].content, /FTS fallback/);
  assert.equal(rows[0].path, 'docs/alpha.md');
});
