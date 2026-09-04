import {
  endpoint,
  gatewayTimeoutMs,
  isCapabilityEnabled,
  providers,
} from './config.mjs';

export class GatewayError extends Error {
  constructor(message, { status, path } = {}) {
    super(message);
    this.name = 'GatewayError';
    this.status = status;
    this.path = path;
  }
}

async function gatewayRequest(path, body) {
  if (!endpoint.apiKey || !endpoint.baseUrl) {
    throw new GatewayError('endpoint credentials are not configured', {
      path,
    });
  }

  const response = await fetch(`${endpoint.baseUrl}${path}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${endpoint.apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(gatewayTimeoutMs),
  });

  const text = await response.text();
  let payload = null;

  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = { raw: text };
  }

  if (!response.ok) {
    const detail =
      payload?.error?.message ||
      payload?.message ||
      text.slice(0, 240) ||
      response.statusText;
    throw new GatewayError(`endpoint ${path} failed: ${detail}`, {
      status: response.status,
      path,
    });
  }

  return payload;
}

/**
 * Create embeddings for one or more texts.
 * Returns an array aligned to the input order.
 */
export async function createEmbeddings(texts) {
  if (!isCapabilityEnabled('embedding')) {
    return null;
  }

  const inputs = Array.isArray(texts) ? texts : [texts];
  if (!inputs.length) {
    return [];
  }

  const payload = await gatewayRequest('/v1/embeddings', {
    model: providers.embedding,
    input: inputs,
  });

  const rows = [...(payload?.data ?? [])].sort(
    (left, right) => Number(left.index ?? 0) - Number(right.index ?? 0),
  );

  if (rows.length !== inputs.length) {
    throw new GatewayError(
      `Embedding response size mismatch: expected ${inputs.length}, got ${rows.length}`,
      { path: '/v1/embeddings' },
    );
  }

  return rows.map((row) => {
    const vector = row.embedding;
    if (!Array.isArray(vector) || !vector.length) {
      throw new GatewayError('Embedding response missing vector data', {
        path: '/v1/embeddings',
      });
    }
    return vector;
  });
}

/**
 * Rerank documents for a query.
 * `documents` is an array of strings; returns results as
 * [{ index, relevance_score }, ...] ordered by relevance.
 */
export async function rerankDocuments(query, documents, topN = documents.length) {
  if (!isCapabilityEnabled('rerank')) {
    return null;
  }

  if (!documents.length) {
    return [];
  }

  const payload = await gatewayRequest('/v1/rerank', {
    model: providers.rerank,
    query,
    documents,
    top_n: Math.min(topN, documents.length),
  });

  const results = payload?.results;
  if (!Array.isArray(results)) {
    throw new GatewayError('Rerank response missing results array', {
      path: '/v1/rerank',
    });
  }

  return results.map((result) => ({
    index: Number(result.index),
    relevanceScore: Number(result.relevance_score),
  }));
}

export async function checkGatewayConnectivity() {
  const report = {
    credentials: Boolean(endpoint.apiKey && endpoint.baseUrl),
    baseUrl: endpoint.baseUrl,
    providers: { ...providers },
    embeddings: { enabled: isCapabilityEnabled('embedding'), ok: false },
    rerank: { enabled: isCapabilityEnabled('rerank'), ok: false },
  };

  if (!report.credentials) {
    return report;
  }

  if (report.embeddings.enabled) {
    const vectors = await createEmbeddings(['provider connectivity check']);
    report.embeddings.ok = Array.isArray(vectors) && vectors[0]?.length > 0;
    report.embeddings.dimensions = vectors?.[0]?.length ?? 0;
  }

  if (report.rerank.enabled) {
    const results = await rerankDocuments(
      'capital of France',
      ['Paris is the capital of France.', 'Bananas are yellow.'],
      2,
    );
    report.rerank.ok =
      Array.isArray(results) &&
      results.length > 0 &&
      Number.isFinite(results[0].relevanceScore);
    report.rerank.topIndex = results?.[0]?.index;
  }

  return report;
}
