import { createServer } from 'node:http';
import { stat } from 'node:fs/promises';
import { extname } from 'node:path';
import {
  dashboardPort as port,
  isCapabilityEnabled,
  providers,
} from './config.mjs';
import { database, databasePath, initializeDatabase } from './db.mjs';

const page = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>gritt-cli Local Memory</title>
  <style>
    :root { color-scheme: dark; font-family: system-ui, sans-serif; }
    body { margin: 0; padding: 24px; background: #0b1018; color: #f3f4f6; }
    h1 { margin-top: 0; }
    .grid, .columns { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; }
    .card, .panel { padding: 16px; border: 1px solid #263244; border-radius: 10px; background: #151c27; }
    .value { display: block; margin-top: 8px; font-size: 1.8rem; font-weight: 700; }
    .panel { margin-top: 16px; overflow: auto; }
    .bar { height: 8px; margin: 8px 0; border-radius: 5px; background: #263244; }
    .bar > span { display: block; height: 100%; border-radius: inherit; background: #8db5d8; }
    table { width: 100%; margin-top: 24px; border-collapse: collapse; }
    th, td { padding: 10px 8px; text-align: left; border-bottom: 1px solid #374151; }
    code { color: #93c5fd; }
    .muted { color: #9ca3af; }
  </style>
</head>
<body>
  <h1>gritt-cli Local Memory</h1>
  <p class="muted">● MCP active · Turso/libSQL + FTS5 · <code>${databasePath}</code></p>
  <section id="stats" class="grid"></section>
  <section class="columns">
    <div class="panel"><h2>Collection</h2><div id="collections"></div></div>
    <div class="panel"><h2>Memory status</h2><div id="status"></div></div>
    <div class="panel"><h2>Top file types</h2><div id="types"></div></div>
  </section>
  <section class="columns">
    <div class="panel"><h2>Recently modified</h2><table><thead><tr><th>Path</th><th>Updated</th></tr></thead><tbody id="documents"></tbody></table></div>
    <div class="panel"><h2>Recent index runs</h2><table><thead><tr><th>Started</th><th>Status</th><th>Files</th><th>Completed</th></tr></thead><tbody id="runs"></tbody></table></div>
  </section>
  <script>
    const render = (id, html) => document.getElementById(id).innerHTML = html;
    const escapeHtml = (value) => String(value ?? '').replace(/[&<>"']/g, c => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;'
    }[c]));
    async function refresh() {
      const data = await fetch('/api/overview').then(r => r.json());
      render('stats', [
        ['Index size', data.stats.size_mb + ' MB'],
        ['Files', data.stats.documents],
        ['Chunks', data.stats.chunks],
        ['Coverage', data.stats.coverage + '%'],
        ['Collections', data.stats.collections],
        ['Index runs', data.stats.runs]
      ].map(([label, value]) => '<div class="card"><span class="muted">' + label + '</span><span class="value">' + value + '</span></div>').join(''));
      render('collections', data.collections.map(c => '<p><strong>' + escapeHtml(c.name) + '</strong> <span class="muted">' + c.count + ' files</span></p>').join(''));
      const maxType = Math.max(...data.types.map(t => t.count), 1);
      render('types', data.types.map(t => '<div><span>' + escapeHtml(t.type) + ' <span class="muted">' + t.count + '</span></span><div class="bar"><span style="width:' + (t.count / maxType * 100) + '%"></span></div></div>').join(''));
      render('documents', data.documents.map(d => '<tr><td><code>' + escapeHtml(d.path) + '</code></td><td>' + escapeHtml(d.updated_at) + '</td></tr>').join(''));
      render('runs', data.runs.map(r => '<tr><td>' + escapeHtml(r.started_at) + '</td><td>' + escapeHtml(r.status) + '</td><td>' + r.files_seen + '</td><td>' + escapeHtml(r.completed_at ?? '') + '</td></tr>').join(''));
      render('status', '<p><strong>Retrieval:</strong> ' + escapeHtml(data.status.retrieval) + '</p><p><strong>Embeddings:</strong> ' + data.stats.embeddings + '/' + data.stats.chunks + ' chunks (' + data.stats.coverage + '%)</p><p><strong>Providers:</strong> ' + escapeHtml(data.status.providers) + '</p><p><strong>Network:</strong> ' + escapeHtml(data.status.network) + '</p><p><strong>Database:</strong> ' + escapeHtml(data.stats.size_mb) + ' MB</p>');
    }
    refresh().catch(error => document.body.insertAdjacentHTML('beforeend', '<p>' + escapeHtml(error) + '</p>'));
    setInterval(refresh, 5000);
  </script>
</body>
</html>`;

async function getOverview() {
  await initializeDatabase();
  const [stats, documents, runs, collections, typeRows, databaseInfo] =
    await Promise.all([
      database.execute(`
      SELECT
        (SELECT COUNT(*) FROM documents) AS documents,
        (SELECT COUNT(*) FROM document_chunks) AS chunks,
        (SELECT COUNT(*) FROM document_chunks WHERE embedding IS NOT NULL) AS embeddings,
        (SELECT COUNT(*) FROM index_runs) AS runs
    `),
      database.execute(`
      SELECT path, updated_at,
             EXISTS(
               SELECT 1 FROM document_chunks c
               WHERE c.document_id = documents.id AND c.embedding IS NOT NULL
             ) AS has_embedding
      FROM documents ORDER BY updated_at DESC LIMIT 20
    `),
      database.execute(`
      SELECT started_at, completed_at, files_seen, status
      FROM index_runs ORDER BY id DESC LIMIT 20
    `),
      database.execute(`
      SELECT
        CASE WHEN instr(path, '/') = 0 THEN '(root)' ELSE substr(path, 1, instr(path, '/') - 1) END AS name,
        COUNT(*) AS count
      FROM documents GROUP BY name ORDER BY count DESC LIMIT 10
    `),
      database.execute('SELECT path FROM documents'),
      stat(databasePath),
    ]);
  const summary = stats.rows[0];
  const documentCount = Number(summary.documents ?? 0);
  const chunkCount = Number(summary.chunks ?? 0);
  const embeddingCount = Number(summary.embeddings ?? 0);
  const typeCounts = new Map();
  for (const row of typeRows.rows) {
    const type = extname(String(row.path)).toLowerCase() || '(none)';
    typeCounts.set(type, (typeCounts.get(type) ?? 0) + 1);
  }
  const types = [...typeCounts.entries()]
    .map(([type, count]) => ({ type, count }))
    .sort((left, right) => right.count - left.count)
    .slice(0, 12);

  const embeddingOn = isCapabilityEnabled('embedding');
  const rerankOn = isCapabilityEnabled('rerank');
  const retrieval = embeddingOn
    ? rerankOn
      ? 'FTS5 + vector + rerank'
      : 'FTS5 + vector'
    : 'FTS5 lexical';

  return {
    stats: {
      documents: documentCount,
      chunks: chunkCount,
      embeddings: embeddingCount,
      coverage: chunkCount
        ? Math.round((embeddingCount / chunkCount) * 100)
        : 0,
      runs: Number(summary.runs ?? 0),
      size_mb: (databaseInfo.size / 1024 / 1024).toFixed(1),
      collections: collections.rows.length,
    },
    status: {
      retrieval,
      network: embeddingOn || rerankOn ? 'optional endpoint' : 'local-only',
      providers: [
        `ai=${providers.ai ?? 'off'}`,
        `embedding=${providers.embedding ?? 'off'}`,
        `rerank=${providers.rerank ?? 'off'}`,
      ].join(' · '),
    },
    documents: documents.rows,
    runs: runs.rows,
    collections: collections.rows,
    types,
  };
}

const server = createServer(async (request, response) => {
  if (request.url === '/api/overview') {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(await getOverview()));
    return;
  }

  response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  response.end(page);
});

await initializeDatabase();
server.listen(port, '127.0.0.1', () => {
  console.error(`gritt-cli memory dashboard: http://127.0.0.1:${port}`);
});
