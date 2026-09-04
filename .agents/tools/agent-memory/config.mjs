import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import dotenv from 'dotenv';

export const workspaceRoot = fileURLToPath(new URL('../../../', import.meta.url));
export const agentsRoot = resolve(workspaceRoot, '.agents');
export const brainRoot = resolve(agentsRoot, 'brain');
export const dashboardPort = 8282;
export const embeddingDimensions = 1536;
export const embeddingBatchSize = 32;
export const gatewayTimeoutMs = 30_000;

// Optional provider credentials only. A missing file leaves every provider off.
dotenv.config({ path: resolve(agentsRoot, '.env'), quiet: true });

function resolveProvider(value) {
  const model = value?.trim() ?? '';

  if (model === '' || model.toLowerCase() === 'none') {
    return null;
  }

  return model;
}

function readProviders() {
  return {
    ai: resolveProvider(process.env.AGENT_AI_PROVIDER),
    embedding: resolveProvider(process.env.AGENT_EMBEDDING_PROVIDER),
    rerank: resolveProvider(process.env.AGENT_RERANK_PROVIDER),
  };
}

function readEndpoint() {
  return {
    apiKey: process.env.AGENT_MEMORY_API_KEY?.trim() || null,
    baseUrl: process.env.AGENT_MEMORY_BASE_URL?.trim().replace(/\/$/, '') || null,
  };
}

export const providers = new Proxy(
  {},
  {
    get(_target, property) {
      return readProviders()[property];
    },
    ownKeys() {
      return Reflect.ownKeys(readProviders());
    },
    getOwnPropertyDescriptor(_target, property) {
      const value = readProviders()[property];
      if (value === undefined) {
        return undefined;
      }
      return {
        configurable: true,
        enumerable: true,
        value,
      };
    },
  },
);

export const endpoint = new Proxy(
  {},
  {
    get(_target, property) {
      return readEndpoint()[property];
    },
  },
);

export function isCapabilityEnabled(capability) {
  const current = readEndpoint();
  return Boolean(current.apiKey && current.baseUrl && readProviders()[capability]);
}
