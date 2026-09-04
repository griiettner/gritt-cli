import { isCapabilityEnabled } from './config.mjs';
import { database, initializeDatabase } from './db.mjs';
import { createEmbeddings, rerankDocuments } from './gateway.mjs';

function normalizeQuery(query) {
  return query
    .split(/\s+/)
    .map((term) => term.replace(/[^\p{L}\p{N}_-]/gu, ''))
    .filter(Boolean)
    .map((term) => `"${term.replaceAll('"', '""')}"`)
    .join(' AND ');
}

function mapRow(row, source) {
  return {
    path: String(row.path),
    title: String(row.title),
    heading: row.heading == null ? null : String(row.heading),
    start_line: Number(row.start_line),
    end_line: Number(row.end_line),
    content: String(row.content),
    score: Number(row.score ?? 0),
    chunk_id: Number(row.chunk_id ?? row.id ?? 0),
    source,
  };
}

async function searchLexical(query, limit) {
  const normalizedQuery = normalizeQuery(query);
  if (!normalizedQuery) {
    return [];
  }

  const result = await database.execute({
    sql: `
      SELECT d.path, d.title, c.id AS chunk_id, c.heading, c.start_line, c.end_line,
             c.content, bm25(document_chunks_fts) AS score
      FROM document_chunks_fts
      JOIN document_chunks c ON c.id = document_chunks_fts.rowid
      JOIN documents d ON d.id = c.document_id
      WHERE document_chunks_fts MATCH ?
      ORDER BY score
      LIMIT ?
    `,
    args: [normalizedQuery, limit],
  });

  return result.rows.map((row) => mapRow(row, 'fts'));
}

async function countEmbeddedChunks() {
  const result = await database.execute(
    'SELECT COUNT(*) AS count FROM document_chunks WHERE embedding IS NOT NULL',
  );
  return Number(result.rows[0]?.count ?? 0);
}

async function searchVector(query, limit) {
  if (!isCapabilityEnabled('embedding')) {
    return [];
  }

  if ((await countEmbeddedChunks()) === 0) {
    return [];
  }

  const vectors = await createEmbeddings([query]);
  if (!vectors?.[0]) {
    return [];
  }

  const result = await database.execute({
    sql: `
      SELECT d.path, d.title, c.id AS chunk_id, c.heading, c.start_line, c.end_line,
             c.content,
             vector_distance_cos(c.embedding, vector32(?)) AS score
      FROM document_chunks c
      JOIN documents d ON d.id = c.document_id
      WHERE c.embedding IS NOT NULL
      ORDER BY score
      LIMIT ?
    `,
    args: [JSON.stringify(vectors[0]), limit],
  });

  return result.rows.map((row) => mapRow(row, 'vector'));
}

function mergeCandidates(lexical, vector, limit) {
  const byChunk = new Map();

  for (const row of [...lexical, ...vector]) {
    const existing = byChunk.get(row.chunk_id);
    if (!existing) {
      byChunk.set(row.chunk_id, {
        ...row,
        sources: [row.source],
      });
      continue;
    }

    if (!existing.sources.includes(row.source)) {
      existing.sources.push(row.source);
    }
  }

  return [...byChunk.values()].slice(0, limit);
}

async function applyRerank(query, candidates, limit) {
  if (!isCapabilityEnabled('rerank') || candidates.length < 2) {
    return candidates.slice(0, limit);
  }

  const results = await rerankDocuments(
    query,
    candidates.map((candidate) => candidate.content),
    limit,
  );

  if (!results?.length) {
    return candidates.slice(0, limit);
  }

  const reranked = [];
  for (const result of results) {
    const candidate = candidates[result.index];
    if (!candidate) {
      continue;
    }
    reranked.push({
      ...candidate,
      score: result.relevanceScore,
      source: `${candidate.source}+rerank`,
    });
  }

  // Keep any leftovers that the reranker omitted, in original order.
  for (const candidate of candidates) {
    if (reranked.length >= limit) {
      break;
    }
    if (!reranked.some((row) => row.chunk_id === candidate.chunk_id)) {
      reranked.push(candidate);
    }
  }

  return reranked.slice(0, limit);
}

export async function searchKnowledge(query, limit = 10) {
  await initializeDatabase();

  const candidateLimit = Math.max(limit * 3, 20);
  let candidates = await searchLexical(query, candidateLimit);

  if (isCapabilityEnabled('embedding')) {
    try {
      const vectorHits = await searchVector(query, candidateLimit);
      candidates = mergeCandidates(candidates, vectorHits, candidateLimit);
    } catch (error) {
      console.error(`Vector search unavailable; using FTS5 only: ${error.message}`);
    }
  }

  if (!candidates.length) {
    // Pure semantic queries with no lexical hits can still recover via vectors.
    if (isCapabilityEnabled('embedding')) {
      try {
        candidates = await searchVector(query, limit);
      } catch {
        return [];
      }
    } else {
      return [];
    }
  }

  try {
    candidates = await applyRerank(query, candidates, limit);
  } catch (error) {
    console.error(`Rerank unavailable; returning unreordered candidates: ${error.message}`);
    candidates = candidates.slice(0, limit);
  }

  return candidates.slice(0, limit);
}
