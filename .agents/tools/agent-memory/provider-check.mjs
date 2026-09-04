#!/usr/bin/env node
import { checkGatewayConnectivity } from './gateway.mjs';
import { isCapabilityEnabled, providers } from './config.mjs';

const report = await checkGatewayConnectivity();

console.log('gritt-cli agent-memory provider check');
console.log(`Credentials: ${report.credentials ? 'present' : 'missing'}`);
console.log(`Base URL: ${report.baseUrl ?? '(none)'}`);
console.log(
  `Providers: ai=${providers.ai ?? 'off'} embedding=${providers.embedding ?? 'off'} rerank=${providers.rerank ?? 'off'}`,
);

if (!report.credentials) {
  console.log('Result: offline baseline (no endpoint credentials). FTS5-only mode.');
  process.exit(0);
}

let failed = false;

if (isCapabilityEnabled('embedding')) {
  if (report.embeddings.ok) {
    console.log(
      `Embeddings: ok (${report.embeddings.dimensions} dimensions)`,
    );
  } else {
    console.error('Embeddings: failed');
    failed = true;
  }
} else {
  console.log('Embeddings: disabled');
}

if (isCapabilityEnabled('rerank')) {
  if (report.rerank.ok) {
    console.log(`Rerank: ok (top index ${report.rerank.topIndex})`);
  } else {
    console.error('Rerank: failed');
    failed = true;
  }
} else {
  console.log('Rerank: disabled');
}

process.exit(failed ? 1 : 0);
