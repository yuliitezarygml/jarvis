import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, renameSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { sources, sections } from './sources.mjs';

const root = resolve(process.argv[2] || '.kb/sources');
mkdirSync(root, { recursive: true });
function git(args, options = {}) {
  return execFileSync('git', ['-c', 'core.autocrlf=false', ...args], {
    encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, timeout: 300_000, ...options,
    env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GCM_INTERACTIVE: 'Never' },
  }).trim();
}
const report = [];
for (const source of sources) {
  const {repo, branch, kind} = source;
  const url = 'https://github.com/' + repo + '.git';
  const dir = join(root, repo.split('/')[1]);
  try {
    const refs = git(['ls-remote', '--exit-code', '--heads', url, 'refs/heads/' + branch]);
    if (!refs) throw Error('Branch missing: ' + branch);
    if (!existsSync(join(dir, '.git'))) {
      git(['clone', '--depth=1', '--filter=blob:none', '--sparse', '--no-checkout', '--branch', branch, url, dir]);
    }
    const run = (args, options) => git(['-C', dir, ...args], options);
    if (run(['remote', 'get-url', 'origin']) !== url || run(['branch', '--show-current']) !== branch)
      throw Error('Unexpected origin or branch in source cache');
    // On the first --no-checkout clone there is no working tree yet.
    if (existsSync(join(dir, '.git', 'index'))) {
      if (run(['status', '--porcelain'])) throw Error('Source cache has local changes');
      run(['pull', '--ff-only', '--depth=1']);
    }
    const files = run(['ls-tree', '-r', '--name-only', 'HEAD']).split(/\r?\n/);
    const dirs = sections(kind, files);
    const licenses = files.filter(p => /^(license(?:[.-](?:code|mit|cc-by[^.]*))?(?:\.md|\.txt)?|copying(?:\.md)?)$/i.test(p));
    if (!licenses.length) throw Error('No license at repository root');
    const patterns = [...licenses.map(p => '/' + p), ...dirs.flatMap(p => ['/' + p + '/*.md','/' + p + '/**/*.md']), '!**/media/**', '!**/images/**'];
    run(['sparse-checkout', 'set', '--no-cone', '--stdin'], { input: patterns.join('\n') + '\n' });
    run(['checkout', branch]);
    report.push({...source, url, sha:run(['rev-parse','HEAD']), dirs, licenses});
    console.log(repo, dirs.join(', '));
  } catch (err) {
    // Do not publish a cache manifest that silently substitutes an old checkout.
    report.push({...source, url, error: 'Fetch failed; see git diagnostics'});
    console.error(repo, err.message.split('\n')[0]);
  }
}
const manifest = join(root, 'sources.json');
writeFileSync(manifest + '.tmp', JSON.stringify(report, null, 2));
renameSync(manifest + '.tmp', manifest);
if (report.some(s => s.error)) process.exitCode = 1;
