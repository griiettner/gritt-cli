import assert from 'node:assert/strict';
import test from 'node:test';

test('gateway embeds and reranks through mocked endpoints', async (t) => {
  process.env.AGENT_MEMORY_API_KEY = 'sk-test';
  process.env.AGENT_MEMORY_BASE_URL = 'https://example.test';
  process.env.AGENT_EMBEDDING_PROVIDER = 'text-embedding-3-small';
  process.env.AGENT_RERANK_PROVIDER = 'rerank-3.5';

  const originalFetch = globalThis.fetch;
  const calls = [];

  globalThis.fetch = async (url, init = {}) => {
    calls.push({ url: String(url), body: init.body ? JSON.parse(init.body) : null });
    if (String(url).endsWith('/v1/embeddings')) {
      return new Response(
        JSON.stringify({
          data: [
            { index: 1, embedding: Array(4).fill(0.2) },
            { index: 0, embedding: Array(4).fill(0.1) },
          ],
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    }

    if (String(url).endsWith('/v1/rerank')) {
      return new Response(
        JSON.stringify({
          results: [
            { index: 1, relevance_score: 0.9 },
            { index: 0, relevance_score: 0.1 },
          ],
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    }

    return new Response('not found', { status: 404 });
  };

  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  const { createEmbeddings, rerankDocuments } = await import(
    `./gateway.mjs?test=${Date.now()}`
  );

  const vectors = await createEmbeddings(['one', 'two']);
  assert.equal(vectors.length, 2);
  assert.deepEqual(vectors[0], Array(4).fill(0.1));
  assert.deepEqual(vectors[1], Array(4).fill(0.2));
  assert.equal(calls[0].url, 'https://example.test/v1/embeddings');
  assert.equal(calls[0].body.model, 'text-embedding-3-small');

  const ranked = await rerankDocuments('q', ['a', 'b'], 2);
  assert.deepEqual(ranked, [
    { index: 1, relevanceScore: 0.9 },
    { index: 0, relevanceScore: 0.1 },
  ]);
  assert.equal(calls[1].url, 'https://example.test/v1/rerank');
  assert.equal(calls[1].body.model, 'rerank-3.5');
});

test('gateway stays off when providers are absent', async () => {
  delete process.env.AGENT_MEMORY_API_KEY;
  delete process.env.AGENT_MEMORY_BASE_URL;
  delete process.env.AGENT_EMBEDDING_PROVIDER;
  delete process.env.AGENT_RERANK_PROVIDER;

  const { createEmbeddings, rerankDocuments } = await import(
    `./gateway.mjs?off=${Date.now()}`
  );

  assert.equal(await createEmbeddings(['x']), null);
  assert.equal(await rerankDocuments('q', ['a']), null);
});
