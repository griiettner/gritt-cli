const MAX_LINES = 80;
const OVERLAP_LINES = 10;

function headingForLine(line) {
  const match = line.match(/^#{1,6}\s+(.+?)\s*#*$/);
  return match?.[1] ?? null;
}

function splitLines(lines, heading, startLine) {
  const chunks = [];
  let cursor = 0;

  while (cursor < lines.length) {
    const end = Math.min(cursor + MAX_LINES, lines.length);
    chunks.push({
      heading,
      startLine: startLine + cursor,
      endLine: startLine + end - 1,
      content: lines.slice(cursor, end).join('\n').trim(),
    });
    if (end === lines.length) {
      break;
    }
    cursor = end - OVERLAP_LINES;
  }

  return chunks.filter((chunk) => chunk.content);
}

export function chunkDocument(content) {
  const lines = content.split(/\r?\n/);
  const chunks = [];
  let sectionLines = [];
  let sectionHeading = null;
  let sectionStart = 1;

  const flushSection = () => {
    chunks.push(...splitLines(sectionLines, sectionHeading, sectionStart));
    sectionLines = [];
  };

  lines.forEach((line, index) => {
    const heading = headingForLine(line);
    if (heading && sectionLines.length) {
      flushSection();
      sectionHeading = heading;
      sectionStart = index + 1;
    } else if (heading) {
      sectionHeading = heading;
      sectionStart = index + 1;
    }
    sectionLines.push(line);
  });
  flushSection();

  return chunks;
}
