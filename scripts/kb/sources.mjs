export const sources = [
  { repo: 'MicrosoftDocs/PowerShell-Docs', branch: 'main', kind: 'powershell' },
  { repo: 'MicrosoftDocs/windows-powershell-docs', branch: 'main', kind: 'modules' },
  { repo: 'MicrosoftDocs/windowsserverdocs', branch: 'main', kind: 'commands' },
  { repo: 'MicrosoftDocs/windows-itpro-docs', branch: 'public', kind: 'itpro' },
  { repo: 'tldr-pages/tldr', branch: 'main', kind: 'tldr' },
];

export function sections(kind, paths) {
  const roots = paths.filter(p => p.endsWith('.md'));
  function latest(pattern) {
    const choices = roots.map(p => p.match(pattern)?.[1]).filter(Boolean);
    return [...new Set(choices)].sort((a, b) => a.localeCompare(b, 'en', { numeric: true })).at(-1);
  }
  let selected;
  switch (kind) {
    case 'powershell': {
      const newest = latest(/^(reference\/7\.\d+)\//);
      if (!newest) throw Error('PowerShell 7.x reference missing');
      selected = ['reference/5.1', newest];
      break;
    }
    case 'modules': {
      const edition = latest(/^(docset\/winserver\d+-ps)\//);
      if (!edition) throw Error('Windows module docset missing');
      selected = ['NetAdapter', 'NetTCPIP', 'DnsClient', 'Storage', 'ScheduledTasks', 'PrintManagement']
        .map(module => edition + '/' + module);
      break;
    }
    case 'commands': selected = ['WindowsServerDocs/administration/windows-commands']; break;
    case 'itpro': selected = ['windows/security', 'windows/configuration', 'windows/client-management', 'windows/deployment/update']; break;
    case 'tldr': selected = ['pages/windows', 'pages/common', 'pages.ru/windows', 'pages.ru/common']; break;
    default: throw Error('Unknown source');
  }
  const found = selected.filter(dir => roots.some(p => p.startsWith(dir + '/')));
  if (!found.length || (kind === 'powershell' && found.length !== selected.length))
    throw Error('Unexpected repository structure: ' + selected.join(', '));
  return found;
}
