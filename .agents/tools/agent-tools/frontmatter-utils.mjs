import { pathToFileURL } from 'node:url';
import { parseCli, runMain } from './lib/cli.mjs';
import { exists, readText } from './lib/fs-utils.mjs';

export const LIST_FIELDS = new Set([
  'dependencies',
  'areas',
  'skills',
  'tags',
  'read_when',
  'chain_children',
]);

export const SUPPORTED_FIELDS = new Set([
  'id',
  'title',
  'type',
  'artifact',
  'status',
  'date',
  'related_ticket',
  'owner',
  'namespace',
  'priority',
  'created',
  'updated',
  'chain_role',
  'chain_parent',
  ...LIST_FIELDS,
]);

export class FrontmatterError {
  constructor(path, message) {
    this.path = path;
    this.message = message;
  }

  render() {
    return `${this.path}: ${this.message}`;
  }
}

export async function loadFrontmatter(filePath) {
  if (!(await exists(filePath))) {
    return { metadata: {}, errors: [] };
  }
  try {
    return parseFrontmatterDocument(filePath, await readText(filePath));
  } catch (error) {
    return {
      metadata: {},
      errors: [
        new FrontmatterError(filePath, `cannot read file: ${error.message}`),
      ],
    };
  }
}

export function parseFrontmatterDocument(filePath, content) {
  if (!content.startsWith('---\n')) {
    return { metadata: {}, errors: [] };
  }
  const end = content.indexOf('\n---', 4);
  if (end === -1) {
    return {
      metadata: {},
      errors: [
        new FrontmatterError(
          filePath,
          'frontmatter starts with `---` but does not close',
        ),
      ],
    };
  }
  return parseFrontmatterBlock(filePath, content.slice(4, end));
}

export function parseFrontmatterBlock(filePath, frontmatter) {
  const metadata = {};
  const errors = [];
  let currentList = null;

  for (const [offset, rawLine] of frontmatter.split(/\r?\n/).entries()) {
    const lineNumber = offset + 2;
    if (!rawLine.trim()) {
      continue;
    }
    if (rawLine.startsWith('  - ')) {
      if (!currentList) {
        errors.push(
          new FrontmatterError(
            filePath,
            `line ${lineNumber}: list item without a list field`,
          ),
        );
        continue;
      }
      const value = rawLine.slice(4).trim();
      if (!value || value.includes(':')) {
        errors.push(
          new FrontmatterError(
            filePath,
            `line ${lineNumber}: only scalar list items are supported in \`${currentList}\``,
          ),
        );
        continue;
      }
      metadata[currentList] ??= [];
      metadata[currentList].push(cleanScalar(value));
      continue;
    }

    currentList = null;
    if (rawLine.startsWith(' ')) {
      errors.push(
        new FrontmatterError(
          filePath,
          `line ${lineNumber}: nested mappings are not supported in scaffold frontmatter`,
        ),
      );
      continue;
    }

    const separator = rawLine.indexOf(':');
    if (separator === -1) {
      errors.push(
        new FrontmatterError(
          filePath,
          `line ${lineNumber}: expected \`key: value\``,
        ),
      );
      continue;
    }
    const key = rawLine.slice(0, separator).trim();
    const value = rawLine.slice(separator + 1).trim();
    if (!SUPPORTED_FIELDS.has(key)) {
      errors.push(
        new FrontmatterError(
          filePath,
          `line ${lineNumber}: unsupported field \`${key}\``,
        ),
      );
      continue;
    }
    if (LIST_FIELDS.has(key)) {
      const parsed = parseListValue(filePath, lineNumber, key, value);
      errors.push(...parsed.errors);
      if (parsed.errors.length === 0) {
        metadata[key] = parsed.values;
        if (value === '') {
          currentList = key;
        }
      }
      continue;
    }
    if (value.startsWith('[') || value.startsWith('{')) {
      errors.push(
        new FrontmatterError(
          filePath,
          `line ${lineNumber}: structured values are not supported for scalar field \`${key}\``,
        ),
      );
      continue;
    }
    metadata[key] = cleanScalar(value);
  }
  return { metadata, errors };
}

export function parseListValue(filePath, lineNumber, key, value) {
  if (value === '' || value === '[]') {
    return { values: [], errors: [] };
  }
  if (value.startsWith('[') && value.endsWith(']')) {
    const inner = value.slice(1, -1).trim();
    if (!inner) {
      return { values: [], errors: [] };
    }
    const items = inner.split(',').map((item) => item.trim());
    if (items.some((item) => !item)) {
      return {
        values: [],
        errors: [
          new FrontmatterError(
            filePath,
            `line ${lineNumber}: malformed inline list for \`${key}\``,
          ),
        ],
      };
    }
    if (items.some((item) => item.includes(':'))) {
      return {
        values: [],
        errors: [
          new FrontmatterError(
            filePath,
            `line ${lineNumber}: only scalar inline list items are supported for \`${key}\``,
          ),
        ],
      };
    }
    return { values: items.map(cleanScalar), errors: [] };
  }
  return {
    values: [],
    errors: [
      new FrontmatterError(
        filePath,
        `line ${lineNumber}: unsupported list syntax for \`${key}\`; use \`[]\`, \`[a, b]\`, or block list items`,
      ),
    ],
  };
}

export function cleanScalar(value) {
  return value.trim().replace(/^["']|["']$/g, '');
}

async function main() {
  const args = parseCli(process.argv.slice(2), {}, [
    { name: 'path', required: false },
  ]);
  if (args.help || !args.path) {
    console.log(
      'usage: frontmatter-utils.mjs [-h] [path]\n\nParse supported YAML frontmatter and print JSON.',
    );
    return 0;
  }
  const result = await loadFrontmatter(args.path);
  console.log(JSON.stringify(result.metadata, null, 2));
  for (const error of result.errors) {
    console.error(`error: ${error.render()}`);
  }
  return result.errors.length ? 1 : 0;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await runMain(main);
}
