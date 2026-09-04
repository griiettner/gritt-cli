import { spawn } from 'node:child_process';
import net from 'node:net';
import { fileURLToPath } from 'node:url';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';
import { dashboardPort } from './config.mjs';
import { database, initializeDatabase } from './db.mjs';
import { indexWorkspace } from './index.mjs';
import { searchKnowledge } from './search.mjs';

const server = new McpServer({
  name: 'gritt-local-memory',
  version: '1.0.0',
});

function dashboardIsRunning() {
  return new Promise((resolve) => {
    const socket = net.createConnection(
      {
        host: '127.0.0.1',
        port: dashboardPort,
      },
      () => {
        socket.destroy();
        resolve(true);
      },
    );
    socket.on('error', () => resolve(false));
  });
}

async function ensureDashboard() {
  if (await dashboardIsRunning()) {
    return;
  }

  const dashboard = new URL('./dashboard.mjs', import.meta.url);
  const child = spawn(process.execPath, [fileURLToPath(dashboard)], {
    detached: true,
    stdio: 'ignore',
  });
  child.unref();
}

server.registerTool(
  'search_local_memory',
  {
    description:
      'Search the local gritt-cli workspace knowledge index. Always uses SQLite FTS5. When optional optional providers are configured, also uses vector retrieval and reranking, falling back to FTS5 on any failure.',
    inputSchema: {
      query: z.string().min(1),
      limit: z.number().int().min(1).max(50).default(10),
    },
  },
  async ({ query, limit }) => {
    const rows = await searchKnowledge(query, limit);
    const text = rows
      .map(
        (row, index) =>
          `[${index + 1}] ${row.title}${row.heading ? ` — ${row.heading}` : ''}\n` +
          `Source: ${row.path}:${row.start_line}-${row.end_line}\n\n${row.content}`,
      )
      .join('\n\n---\n\n');

    return {
      content: [
        {
          type: 'text',
          text: text || 'No local knowledge matched the query.',
        },
      ],
    };
  },
);

server.registerTool(
  'read_local_memory',
  {
    description:
      'Read one indexed local knowledge document by its workspace-relative path.',
    inputSchema: {
      path: z.string().min(1),
    },
  },
  async ({ path }) => {
    await initializeDatabase();
    const result = await database.execute({
      sql: 'SELECT path, title, content FROM documents WHERE path = ? LIMIT 1',
      args: [path],
    });
    const row = result.rows[0];

    return {
      content: [
        {
          type: 'text',
          text: row
            ? `## ${row.title}\nPath: ${row.path}\n\n${row.content}`
            : `No local document exists at ${path}.`,
        },
      ],
    };
  },
);

await initializeDatabase();
await indexWorkspace();
await ensureDashboard();
const transport = new StdioServerTransport();
await server.connect(transport);
