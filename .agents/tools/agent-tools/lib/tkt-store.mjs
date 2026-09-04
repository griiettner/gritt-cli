import path from 'node:path';
import { isDirectory, listDirectories } from './fs-utils.mjs';

export const SHARED_NAMESPACE = '_shared';
export const TASK_SHARD_SIZE = 25;
export const CHUNK_DIR_RE = /^TKT-\d{4}-\d{4}$/;
export const TICKET_ID_RE = /^TKT-\d{4}$/;
export const QUALIFIED_TICKET_RE = /^([^/]+)\/(TKT-\d{4})$/;
export const NAMESPACE_RE = /^(?:_shared|[A-Za-z0-9](?:[A-Za-z0-9._-]{0,37}[A-Za-z0-9])?)$/;

export function padTicketNumber(value) {
  return String(value).padStart(4, '0');
}

export function isTicketId(ticketId) {
  return TICKET_ID_RE.test(ticketId);
}

export function isChunkDirName(name) {
  return CHUNK_DIR_RE.test(name);
}

export function isNamespaceName(name) {
  return (
    typeof name === 'string' &&
    NAMESPACE_RE.test(name) &&
    !isChunkDirName(name) &&
    name !== '.' &&
    name !== '..'
  );
}

export function ticketNumber(ticketId) {
  return Number(String(ticketId).split('-')[1]);
}

export function chunkName(number) {
  const start = Math.floor((number - 1) / TASK_SHARD_SIZE) * TASK_SHARD_SIZE + 1;
  return `TKT-${padTicketNumber(start)}-${padTicketNumber(start + TASK_SHARD_SIZE - 1)}`;
}

export function parseTicketRef(value) {
  const raw = String(value || '').trim();
  const qualified = QUALIFIED_TICKET_RE.exec(raw);
  if (qualified) {
    return { namespace: qualified[1], ticketId: qualified[2] };
  }
  if (isTicketId(raw)) {
    return { namespace: null, ticketId: raw };
  }
  return null;
}

export function qualifiedTicketId(namespace, ticketId) {
  return `${namespace}/${ticketId}`;
}

export function namespaceRoot(tasksRoot, namespace) {
  return namespace === SHARED_NAMESPACE
    ? tasksRoot
    : path.join(tasksRoot, namespace);
}

export function ticketDir(tasksRoot, namespace, ticketId) {
  return path.join(
    namespaceRoot(tasksRoot, namespace),
    chunkName(ticketNumber(ticketId)),
    ticketId,
  );
}

export async function listNamespaces(tasksRoot) {
  const namespaces = [{ id: SHARED_NAMESPACE, root: tasksRoot }];
  if (!(await isDirectory(tasksRoot))) return namespaces;
  for (const entry of await listDirectories(tasksRoot)) {
    if (isChunkDirName(entry.name) || !isNamespaceName(entry.name)) continue;
    namespaces.push({
      id: entry.name,
      root: path.join(tasksRoot, entry.name),
    });
  }
  return namespaces;
}

export async function iterTicketDirs(tasksRoot) {
  const result = [];
  for (const namespace of await listNamespaces(tasksRoot)) {
    if (!(await isDirectory(namespace.root))) continue;
    for (const chunk of await listDirectories(namespace.root)) {
      if (!isChunkDirName(chunk.name)) continue;
      const chunkDir = path.join(namespace.root, chunk.name);
      for (const ticket of await listDirectories(chunkDir)) {
        if (!isTicketId(ticket.name)) continue;
        result.push({
          dir: path.join(chunkDir, ticket.name),
          ticketId: ticket.name,
          namespace: namespace.id,
        });
      }
    }
  }
  return result;
}

export async function nextTicketNumber(tasksRoot, namespace) {
  const numbers = new Set();
  const root = namespaceRoot(tasksRoot, namespace);
  if (!(await isDirectory(root))) return 1;
  for (const chunk of await listDirectories(root)) {
    if (!isChunkDirName(chunk.name)) continue;
    for (const ticket of await listDirectories(path.join(root, chunk.name))) {
      if (isTicketId(ticket.name)) {
        numbers.add(ticketNumber(ticket.name));
      }
    }
  }
  const highest = Math.max(0, ...numbers);
  const missing = [];
  for (let number = 1; number < highest; number += 1) {
    if (!numbers.has(number)) missing.push(`TKT-${padTicketNumber(number)}`);
  }
  if (missing.length) {
    throw new Error(
      `ticket sequence has missing ids in namespace ${namespace}: ${missing.join(', ')}; restore or explicitly account for the missing ticket before allocating another id`,
    );
  }
  return highest + 1;
}

export async function findTicketDir(
  tasksRoot,
  ticketRef,
  preferredNamespace = null,
) {
  const parsed = parseTicketRef(ticketRef);
  if (!parsed) {
    throw new Error(`invalid ticket id: ${ticketRef}`);
  }
  if (parsed.namespace) {
    if (!isNamespaceName(parsed.namespace)) {
      throw new Error(`invalid ticket namespace: ${parsed.namespace}`);
    }
    const dir = ticketDir(tasksRoot, parsed.namespace, parsed.ticketId);
    if (!(await isDirectory(dir))) {
      throw new Error(
        `ticket folder does not exist: ${parsed.namespace}/${parsed.ticketId}`,
      );
    }
    return { ...parsed, dir };
  }

  const matches = [];
  for (const namespace of await listNamespaces(tasksRoot)) {
    const dir = ticketDir(tasksRoot, namespace.id, parsed.ticketId);
    if (await isDirectory(dir)) {
      matches.push({
        namespace: namespace.id,
        ticketId: parsed.ticketId,
        dir,
      });
    }
  }

  if (preferredNamespace) {
    const preferred = matches.find(
      (match) => match.namespace === preferredNamespace,
    );
    if (preferred) return preferred;
  }
  if (matches.length === 1) return matches[0];
  if (matches.length === 0) {
    throw new Error(`ticket folder does not exist: ${parsed.ticketId}`);
  }
  const options = matches
    .map((match) => qualifiedTicketId(match.namespace, match.ticketId))
    .join(', ');
  throw new Error(
    `ambiguous ticket id ${parsed.ticketId}; use one of: ${options}`,
  );
}
