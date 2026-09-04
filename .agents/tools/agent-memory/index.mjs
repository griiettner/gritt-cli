import { createHash } from 'node:crypto';
import { readFile, readdir } from 'node:fs/promises';
import { basename, extname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chunkDocument } from './chunk.mjs';
import {
  embeddingBatchSize,
  isCapabilityEnabled,
} from './config.mjs';
import {
  database,
  databasePath,
  initializeDatabase,
  workspaceRoot,
} from './db.mjs';
import { createEmbeddings } from './gateway.mjs';

const root = workspaceRoot;
const allowedExtensions = new Set(['.md', '.mdx', '.yaml', '.yml', '.json']);

async function collectFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  const brainDataDirectory = resolve(root, '.agents', 'brain', 'data');

  for (const entry of entries) {
    if (
      entry.name === 'node_modules' ||
      entry.name === '.git' ||
      entry.name === '.nx' ||
      entry.name === '.playwright-mcp' ||
      entry.name === 'dist' ||
      entry.name === 'coverage' ||
      entry.name === '.output'
    ) {
      continue;
    }

    const entryPath = resolve(directory, entry.name);
    if (entry.isDirectory() && entryPath === brainDataDirectory) {
      continue;
    }

    if (entry.isDirectory()) {
      files.push(...(await collectFiles(entryPath)));
    } else if (allowedExtensions.has(extname(entry.name))) {
      files.push(entryPath);
    }
  }

  return files;
}

function vectorLiteral(vector) {
  return JSON.stringify(vector);
}

async function embedPendingChunks() {
  if (!isCapabilityEnabled('embedding')) {
    return { attempted: 0, embedded: 0 };
  }

  const pending = await database.execute(`
    SELECT id, content
    FROM document_chunks
    WHERE embedding IS NULL
    ORDER BY id
  `);

  if (!pending.rows.length) {
    return { attempted: 0, embedded: 0 };
  }

  let embedded = 0;

  for (let offset = 0; offset < pending.rows.length; offset += embeddingBatchSize) {
    const batch = pending.rows.slice(offset, offset + embeddingBatchSize);
    try {
      const vectors = await createEmbeddings(batch.map((row) => String(row.content)));
      if (!vectors) {
        break;
      }

      for (const [index, row] of batch.entries()) {
        await database.execute({
          sql: `
            UPDATE document_chunks
            SET embedding = vector32(?)
            WHERE id = ?
          `,
          args: [vectorLiteral(vectors[index]), Number(row.id)],
        });
        embedded += 1;
      }
    } catch (error) {
      console.error(
        `Embedding batch failed at offset ${offset}; continuing without remaining vectors: ${error.message}`,
      );
      break;
    }
  }

  return { attempted: pending.rows.length, embedded };
}

async function indexFile(filePath) {
  const content = await readFile(filePath, 'utf8');
  const relativePath = relative(root, filePath);
  const title = basename(filePath, extname(filePath));
  const contentHash = createHash('sha256').update(content).digest('hex');
  const sourceMtime = Date.now();

  await database.execute({
    sql: `
      INSERT INTO documents(path, title, content, content_hash, source_mtime)
      VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(path) DO UPDATE SET
        title = excluded.title,
        content = excluded.content,
        content_hash = excluded.content_hash,
        source_mtime = excluded.source_mtime,
        updated_at = CURRENT_TIMESTAMP
      WHERE documents.content_hash <> excluded.content_hash
    `,
    args: [relativePath, title, content, contentHash, sourceMtime],
  });

  const documentResult = await database.execute({
    sql: 'SELECT id FROM documents WHERE path = ?',
    args: [relativePath],
  });
  const documentId = Number(documentResult.rows[0].id);
  const chunks = chunkDocument(content);
  const retainedIndexes = chunks.map((_, index) => index);

  for (const [chunkIndex, chunk] of chunks.entries()) {
    const chunkHash = createHash('sha256').update(chunk.content).digest('hex');
    await database.execute({
      sql: `
        INSERT INTO document_chunks(
          document_id, chunk_index, heading, start_line, end_line, content, content_hash
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(document_id, chunk_index) DO UPDATE SET
          heading = excluded.heading,
          start_line = excluded.start_line,
          end_line = excluded.end_line,
          content = excluded.content,
          content_hash = excluded.content_hash,
          embedding = CASE
            WHEN document_chunks.content_hash = excluded.content_hash
            THEN document_chunks.embedding
            ELSE NULL
          END
      `,
      args: [
        documentId,
        chunkIndex,
        chunk.heading,
        chunk.startLine,
        chunk.endLine,
        chunk.content,
        chunkHash,
      ],
    });
  }

  if (retainedIndexes.length) {
    await database.execute({
      sql: `
        DELETE FROM document_chunks
        WHERE document_id = ?
          AND chunk_index NOT IN (${retainedIndexes.map(() => '?').join(', ')})
      `,
      args: [documentId, ...retainedIndexes],
    });
  } else {
    await database.execute({
      sql: 'DELETE FROM document_chunks WHERE document_id = ?',
      args: [documentId],
    });
  }
}

export async function indexWorkspace() {
  await initializeDatabase();
  await database.execute("INSERT INTO index_runs(status) VALUES ('running')");
  const runResult = await database.execute(
    'SELECT MAX(id) AS id FROM index_runs',
  );
  const runId = Number(runResult.rows[0].id);
  const files = await collectFiles(root);

  try {
    const relativePaths = files.map((file) => relative(root, file));

    if (relativePaths.length) {
      await database.execute({
        sql: `DELETE FROM documents WHERE path NOT IN (${relativePaths.map(() => '?').join(', ')})`,
        args: relativePaths,
      });
    }

    await database.execute(`
      DELETE FROM document_chunks
      WHERE document_id NOT IN (SELECT id FROM documents)
    `);

    for (const file of files) {
      await indexFile(file);
    }

    const embeddingStats = await embedPendingChunks();

    await database.execute(
      "INSERT INTO documents_fts(documents_fts) VALUES ('rebuild')",
    );
    await database.execute(
      "INSERT INTO document_chunks_fts(document_chunks_fts) VALUES ('rebuild')",
    );
    await database.execute({
      sql: `
        UPDATE index_runs
        SET completed_at = CURRENT_TIMESTAMP, files_seen = ?, status = 'completed'
        WHERE id = ?
      `,
      args: [files.length, runId],
    });

    const embeddingNote = isCapabilityEnabled('embedding')
      ? `; embedded ${embeddingStats.embedded}/${embeddingStats.attempted} pending chunks`
      : '';

    console.error(
      `Indexed ${files.length} local knowledge files into ${databasePath}${embeddingNote}`,
    );
  } catch (error) {
    await database.execute({
      sql: `
        UPDATE index_runs
        SET completed_at = CURRENT_TIMESTAMP, files_seen = ?, status = 'failed', error = ?
        WHERE id = ?
      `,
      args: [files.length, String(error), runId],
    });
    throw error;
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))
) {
  await indexWorkspace();
}
