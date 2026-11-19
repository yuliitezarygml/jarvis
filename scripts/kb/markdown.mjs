// The Microsoft docsets use top-level scalar metadata and, occasionally, YAML lists.
// Preserve other metadata verbatim with the document, including author attribution.
export function parseMarkdown(raw) {
  let body = raw.replace(/^\uFEFF/, '').replaceAll('\r\n', '\n');
  const front = /^---\n([\s\S]*?)\n---(?:\n|$)/.exec(body);
  const meta = {};
  if (front) {
    body = body.slice(front[0].length);
    for (const line of front[1].split('\n')) {
      const field = /^([\w. -]+):\s*(.*?)\s*$/.exec(line);
      if (field) meta[field[1].toLowerCase()] = field[2].replace(/^(['"])(.*)\1$/, '$2');
    }
  }
  const sections = [];
  const headings = [];
  let lines = [], fence = '', title = meta.title || '';
  function flush() {
    const text = lines.join('\n').trim();
    if (text) sections.push({ heading: headings.filter(Boolean).join(' / ') || title, text });
    lines = [];
  }
  for (const line of body.split('\n')) {
    const marker = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      lines.push(line);
      if (marker && marker[1][0] === fence[0] && marker[1].length >= fence.length && !marker[2].trim()) fence = '';
    } else if (marker) {
      fence = marker[1];
      lines.push(line);
    } else {
      const h = /^ {0,3}(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
      if (!h) { lines.push(line); continue; }
      flush();
      headings.length = h[1].length;
      headings[h[1].length - 1] = h[2];
      if (!title && h[1] === '#') title = h[2];
    }
  }
  flush();
  return { title, meta, front: front?.[1] || '', sections };
}

export function applicability(kind, front, parts) {
  const applies = [front.match(/^applies[- ]?to:\s*([^\n]*(?:\n[ \t]*-[^\n]+)*)/im)?.[1],
    ...parts.flatMap(p => p.text.match(/^.*(?:Applies to:|!INCLUDE\s*\[applies-to).*$/gim) || [])].join(' ');
  const names = [...new Set(applies.match(/Windows\s+(?:10|11|Server(?:\s+\d{4})?)/gi) || [])];
  return names.join(', ') || (['modules','commands'].includes(kind) ? 'Server' : 'unknown');
}
