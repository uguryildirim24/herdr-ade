// Trusted Pi tool backend. Never load this from the worktree or merge project policy.
// Pi/provider/extensions stay trusted on the host. Ordinary tool effects execute
// inside namespaces; the fixed authorized seal bridge is the exception below.
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { Type } from '@sinclair/typebox';

export const policy = Object.freeze(__ADE_EXECUTION_POLICY__);
const text = (s) => ({ content: [{ type: 'text', text: s }], details: undefined });
const cleanEnv = { PATH: '/usr/bin:/bin', HOME: '/nonexistent', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
const hostGit = (args) => execFileSync('/usr/bin/git', ['-c', 'core.hooksPath=/dev/null', '-C', policy.cwd, ...args], { env: cleanEnv, encoding: 'utf8' }).trim();
let initialized;

function validatePolicy() {
  if (![policy.cwd, policy.root, policy.state, policy.ade].every((value) => typeof value === 'string' && path.isAbsolute(value)) || !policy.branch) throw new Error('execution_policy_invalid: missing absolute binding');
}

// A scratch ADE root under /tmp could otherwise be recreated in the private
// tmpfs: harmless to the host, but falsely reported as a successful control
// write. Refuse control-root destinations explicitly; namespace isolation is
// still the enforcement boundary for symlinks and all ordinary tool effects.
export function validateMutationPath(filename) {
  const target = path.resolve(policy.cwd, filename);
  const inside = (base) => {
    const relative = path.relative(base, target);
    return relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith('..' + path.sep));
  };
  if (inside(policy.root) && !inside(policy.cwd)) throw new Error('execution_control_write_denied: ADE control records are not lane files');
}

function setup() {
  if (initialized) return;
  validatePolicy();
  fs.mkdirSync(policy.state, { recursive: true, mode: 0o700 });
  const relative = path.relative(fs.realpathSync(policy.cwd), fs.realpathSync(policy.state));
  if (!relative.startsWith('..' + path.sep) && relative !== '..' && !path.isAbsolute(relative)) throw new Error('execution_policy_invalid: backend state is exposed inside the worktree');
  for (const dir of ['build', 'home', 'empty', 'home/.cargo/registry']) fs.mkdirSync(path.join(policy.state, dir), { recursive: true });
  const git = path.join(policy.state, 'git');
  const objects = fs.realpathSync(hostGit(['rev-parse', '--path-format=absolute', '--git-path', 'objects']));
  if (!fs.existsSync(git)) {
    execFileSync('/usr/bin/git', ['init', '--bare', git], { env: cleanEnv });
    fs.writeFileSync(path.join(git, 'objects/info/alternates'), objects + '\n');
    execFileSync('/usr/bin/git', ['--git-dir=' + git, 'update-ref', 'refs/heads/' + policy.branch, hostGit(['rev-parse', 'HEAD'])], { env: cleanEnv });
    fs.writeFileSync(path.join(git, 'HEAD'), 'ref: refs/heads/' + policy.branch + '\n');
    fs.writeFileSync(path.join(git, 'info/exclude'), '.herdr-project/\n');
    execFileSync('/usr/bin/git', ['--git-dir=' + git, 'read-tree', 'HEAD'], { env: cleanEnv });
  }
  fs.writeFileSync(path.join(policy.state, 'git-pointer'), 'gitdir: /lane-git\n');
  fs.writeFileSync(path.join(policy.state, 'git-config'), '[core]\nrepositoryformatversion = 0\nbare = false\nhooksPath = /empty\n[user]\nname = ADE lane\nemail = lane@localhost\n');
  fs.writeFileSync(path.join(policy.state, 'alternates'), '/base-objects\n');
  let toolchain;
  try { toolchain = execFileSync('rustc', ['--print', 'sysroot'], { cwd: policy.state, encoding: 'utf8' }).trim(); } catch { /* Rust is optional. */ }
  // Expose executable/package files, never the account's bin/home/config tree.
  const node = fs.realpathSync(process.execPath);
  const npm = [path.resolve(path.dirname(node), '../lib/node_modules/npm'), '/usr/share/nodejs/npm'].find((dir) => fs.existsSync(path.join(dir, 'bin/npm-cli.js')));
  const uv = (process.env.PATH || '').split(path.delimiter).map((dir) => path.join(dir, 'uv')).find((file) => { try { return fs.statSync(file).isFile() && (fs.statSync(file).mode & 0o111); } catch { return false; } });
  fs.writeFileSync(path.join(policy.state, 'npm-launcher'), '#!/bin/sh\nexec /tool-bin/node /npm/bin/npm-cli.js "$@"\n', { mode: 0o755 });
  initialized = { git, objects, toolchain, node, npm, uv: uv && fs.realpathSync(uv), directoryGit: fs.lstatSync(path.join(policy.cwd, '.git')).isDirectory() };
}

export function sandboxArgs() {
  setup();
  const args = ['--unshare-all', '--die-with-parent', '--new-session', '--cap-drop', 'ALL', '--clearenv'];
  if (policy.network === 'allowed') args.splice(1, 0, '--share-net');
  for (const dir of ['/usr', '/bin', '/lib', '/lib64']) if (fs.existsSync(dir)) args.push('--ro-bind', dir, dir);
  for (const file of ['/etc/ld.so.cache', '/etc/alternatives', '/etc/resolv.conf', '/etc/hosts', '/etc/nsswitch.conf', '/etc/ssl/certs', '/etc/pki/ca-trust/extracted', '/etc/pki/tls/certs']) if (fs.existsSync(file)) args.push('--ro-bind', file, file);
  args.push('--ro-bind', initialized.node, '/tool-bin/node');
  if (initialized.npm) args.push('--ro-bind', initialized.npm, '/npm', '--ro-bind', path.join(policy.state, 'npm-launcher'), '/tool-bin/npm');
  if (initialized.uv) args.push('--ro-bind', initialized.uv, '/tool-bin/uv');
  args.push('--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--ro-bind', path.join(policy.state, 'empty'), '/empty',
    '--bind', policy.cwd, policy.cwd, '--bind', path.join(policy.state, 'build'), '/build',
    '--bind', path.join(policy.state, 'home'), '/home/lane', '--ro-bind', initialized.objects, '/base-objects');
  const viewGit = initialized.directoryGit ? path.join(policy.cwd, '.git') : '/lane-git';
  args.push('--bind', initialized.git, viewGit,
    '--ro-bind', path.join(policy.state, 'git-config'), viewGit + '/config',
    '--ro-bind', path.join(policy.state, 'empty'), viewGit + '/hooks',
    '--ro-bind', path.join(policy.state, 'alternates'), viewGit + '/objects/info/alternates');
  if (!initialized.directoryGit) args.push('--ro-bind', path.join(policy.state, 'git-pointer'), path.join(policy.cwd, '.git'));
  if (initialized.toolchain) args.push('--ro-bind', initialized.toolchain, '/toolchain');
  const registry = path.join(process.env.HOME || '', '.cargo/registry');
  if (fs.existsSync(registry)) args.push('--ro-bind', registry, '/cargo-base/registry');
  args.push('--setenv', 'PATH', '/home/lane/.cargo/bin:/tool-bin:/toolchain/bin:/usr/bin:/bin', '--setenv', 'HOME', '/home/lane',
    '--setenv', 'CARGO_HOME', '/home/lane/.cargo', '--setenv', 'CARGO_TARGET_DIR', '/build', '--setenv', 'GIT_CONFIG_NOSYSTEM', '1',
    '--setenv', 'GIT_CONFIG_GLOBAL', '/dev/null', '--setenv', 'GIT_TERMINAL_PROMPT', '0', '--setenv', 'GIT_EDITOR', 'true', '--chdir', policy.cwd, '--remount-ro', '/');
  return args;
}

export function runSandbox(argv, signal, timeout = 600, binary = false) {
  return new Promise((resolve, reject) => {
    const child = spawn('/usr/bin/bwrap', [...sandboxArgs(), ...argv], { env: cleanEnv, stdio: ['ignore', 'pipe', 'pipe'], signal });
    const stdout = []; const stderr = []; let bytes = 0; let overflow = false;
    const limit = binary ? 200 * 1024 * 1024 : 1024 * 1024;
    const collect = (chunks) => (chunk) => { bytes += chunk.length; if (bytes <= limit) chunks.push(chunk); else { overflow = true; child.kill('SIGKILL'); } };
    child.stdout.on('data', collect(stdout)); child.stderr.on('data', collect(stderr));
    const timer = setTimeout(() => child.kill('SIGKILL'), Math.min(Math.max(timeout, 1), 3600) * 1000);
    child.on('error', (e) => { clearTimeout(timer); reject(e); });
    child.on('close', (code) => {
      clearTimeout(timer);
      if (code !== 0 || overflow) reject(new Error(`isolated command exit ${code}${overflow ? ' (output limit)' : ''}: ${Buffer.concat(stderr).toString()}${binary ? '' : Buffer.concat(stdout).toString()}`));
      else resolve(binary ? Buffer.concat(stdout) : Buffer.concat([...stdout, ...stderr]).toString());
    });
  });
}

// No arbitrary host command, argument, report path, SHA, remote or credential API.
// Candidate transfer is data, created INSIDE the boundary; private git config,
// refs, alternates, hooks and worktree scripts are never interpreted by host Git.
export async function seal(action, reason, signal) {
  validatePolicy();
  if (!['skill', 'done', 'waiting', 'failed'].includes(action)) throw new Error('unsupported seal action');
  if (action === 'done') {
    if ((await runSandbox(['/usr/bin/git', 'status', '--porcelain'], signal)).trim()) throw new Error('worktree_dirty: commit lane changes before ha done');
    const bundle = await runSandbox(['/usr/bin/git', 'bundle', 'create', '-', 'HEAD'], signal, 600, true);
    const target = path.join(policy.state, 'candidate.bundle');
    fs.writeFileSync(target, bundle, { mode: 0o600 });
    hostGit(['fetch', '--update-head-ok', '--no-tags', '--no-write-fetch-head', target, 'HEAD:refs/heads/' + policy.branch]);
    hostGit(['read-tree', 'HEAD']);
  }
  const args = ['--root', policy.root, action === 'skill' ? 'skill' : action];
  if (action === 'skill') args.push('--', policy.role || 'lane');
  if (action === 'waiting' || action === 'failed') {
    if (typeof reason !== 'string' || reason.length === 0 || reason.length > 16000) throw new Error('a bounded reason is required');
    args.push('--', reason);
  }
  return execFileSync(policy.ade, args, { cwd: policy.cwd, encoding: 'utf8', timeout: 120000,
    env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' }, maxBuffer: 1024 * 1024 });
}

export default function (pi) {
  validatePolicy();
  // --no-builtin-tools starts with host tools INACTIVE even if we cannot load.
  // --no-tools would prevent activating extension tools too (Pi 0.99.1).
  const names = ['bash', 'read', 'write', 'edit', 'ade'];
  pi.registerFlag('ade-execution-probe', { description: 'Report bounded CLI tool provenance without a model request', type: 'boolean', default: false });
  pi.on('session_start', (_event, ctx) => {
    pi.setActiveTools(names);
    if (pi.getFlag('ade-execution-probe')) {
      // Run after all trusted session_start handlers, so the observed loadout is
      // what the first turn would actually see, not just our requested names.
      setImmediate(() => {
        const active = pi.getActiveTools();
        const tools = pi.getAllTools().filter(tool => active.includes(tool.name));
        console.error('ADE_BOUNDARY_TOOLS=' + JSON.stringify(tools.map(tool => ({ name: tool.name, source: tool.sourceInfo }))));
        ctx.shutdown();
      });
    }
  });
  pi.on('tool_call', (event) => !names.includes(event.toolName) ? { block: true, reason: 'tool has no isolated backend' } : undefined);
  pi.registerTool({ name: 'bash', label: 'bash (isolated)', description: `Run shell/file/build commands in the lane filesystem boundary. No host credentials. Tool network ${policy.network === 'allowed' ? 'allowed' : 'denied'}. Use ade for skill/sealing, not ha in bash.`,
    parameters: Type.Object({ command: Type.String(), timeout: Type.Optional(Type.Number()) }),
    async execute(_id, params, signal) { return text(await runSandbox(['/bin/bash', '--noprofile', '--norc', '-c', params.command], signal, params.timeout)); } });
  // All file operations run INSIDE the same namespace as bash. Passing JSON
  // as an argv value (never shell text) also preserves literal paths/content.
  pi.registerTool({ name: 'read', label: 'read (isolated)', description: 'Read a text file inside the lane boundary, optionally by one-based line offset and limit.',
    parameters: Type.Object({ path: Type.String(), offset: Type.Optional(Type.Number()), limit: Type.Optional(Type.Number()) }),
    async execute(_id, params, signal) { return text(await runSandbox(['/tool-bin/node', '-e', "const fs=require('fs'),p=JSON.parse(process.argv[1]); const lines=fs.readFileSync(p.path,'utf8').split('\\n'); const start=Math.max(0,(p.offset||1)-1); console.log(lines.slice(start,start+Math.min(p.limit||2000,2000)).join('\\n').slice(0,50000));", JSON.stringify(params)], signal)); } });
  pi.registerTool({ name: 'write', label: 'write (isolated)', executionMode: 'sequential', description: 'Write a file inside the lane boundary, creating parent directories.',
    parameters: Type.Object({ path: Type.String(), content: Type.String() }),
    async execute(_id, params, signal) { validateMutationPath(params.path); return text(await runSandbox(['/tool-bin/node', '-e', "const fs=require('fs'),path=require('path'),p=JSON.parse(process.argv[1]); fs.mkdirSync(path.dirname(p.path),{recursive:true}); fs.writeFileSync(p.path,p.content); console.log('Wrote '+p.path);", JSON.stringify(params)], signal)); } });
  pi.registerTool({ name: 'edit', label: 'edit (isolated)', executionMode: 'sequential', description: 'Replace one unique exact text match inside the lane boundary.',
    parameters: Type.Object({ path: Type.String(), oldText: Type.String(), newText: Type.String() }),
    async execute(_id, params, signal) { validateMutationPath(params.path); return text(await runSandbox(['/tool-bin/node', '-e', "const fs=require('fs'),p=JSON.parse(process.argv[1]),s=fs.readFileSync(p.path,'utf8'); const i=s.indexOf(p.oldText); if(!p.oldText||i<0||s.indexOf(p.oldText,i+p.oldText.length)>=0) throw Error('oldText must match exactly once'); fs.writeFileSync(p.path,s.slice(0,i)+p.newText+s.slice(i+p.oldText.length)); console.log('Edited '+p.path);", JSON.stringify(params)], signal)); } });
  pi.registerTool({ name: 'ade', label: 'ADE authorized seal', executionMode: 'sequential', description: 'Bound lane skill or ha done/waiting/failed. done transfers the private lane commit, then runs the existing authorized seal/publish path. No other control or publication operation.',
    parameters: Type.Object({ action: Type.Union(['skill', 'done', 'waiting', 'failed'].map((action) => Type.Literal(action))), reason: Type.Optional(Type.String()) }),
    async execute(_id, params, signal) { return text(await seal(params.action, params.reason, signal)); } });
  pi.on('user_bash', async (event) => ({ result: { output: await runSandbox(['/bin/bash', '--noprofile', '--norc', '-c', event.command]), exitCode: 0, cancelled: false, truncated: false } }));
  pi.on('before_agent_start', (event) => ({ systemPrompt: event.systemPrompt + '\nADE execution: Linux bubblewrap tool boundary. Only isolated bash, read, write, edit and the narrow ade bridge are available. Use ade(action=skill) for the lane skill; ade(action=done) is ha done. Provider/auth and trusted runtime stay outside; no worktree extensions or MCP load. ' + (policy.network === 'allowed' ? 'Tool network is allowed for dependency acquisition and research. ' : 'Tool network is denied by the frozen recipe. ') + 'Cargo/npm/uv use the writable lane-private home/cache. Build/artifact state persists in /build and the worktree.' }));
}
