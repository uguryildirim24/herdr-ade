#!/usr/bin/env node
// A plumbing lane, not a model simulator. Explicit lifecycle reports use the
// same pi hook surface as a real lane. Never for idle-only/model findings.
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const cp = require('child_process');
const args = process.argv.slice(2);
// The installed herdr also checks the foreground process name at prompt time.
// This is a declared scripted pi stand-in, not a real provider-backed agent.
process.title = 'pi';
if (args.includes('--wall-check')) {
  if (args.includes('--provider')) {
    const value = flag => args[args.indexOf(flag)+1];
    const result = cp.spawnSync('herdr-pi',['check',value('--provider'),'--model',value('--model')],{stdio:'inherit'});
    process.exit(result.status ?? 1);
  }
  console.log('OK'); process.exit(0);
}
const home = process.env.HOME;
if (!['/home/wall', '/home/wall/box'].includes(home)) throw Error('sandbox HOME required');
const pane = process.env.HERDR_PANE_ID;
if (!pane) throw Error('herdr pane required');
const session = crypto.randomUUID();
let sequence = 0;
const sessionPath = path.join(home, 'runs', session + '.jsonl');
fs.mkdirSync(path.dirname(sessionPath), {recursive:true});
const event = (phase, extra={}) => {
  const row = {at:new Date().toISOString(), pid:process.pid, pane, session, phase, ...extra};
  fs.appendFileSync(sessionPath, JSON.stringify(row)+'\n');
  console.log(JSON.stringify(row));
};
const report = state => {
  const result = cp.spawnSync('herdr', ['pane','report-agent',pane,'--source','pi',
    '--agent','pi','--state',state,'--seq',String(++sequence),
    '--agent-session-id',session,'--agent-session-path',sessionPath], {encoding:'utf8'});
  if (result.status !== 0) throw Error(result.stderr || result.stdout);
};
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function checkpoint(phase, control) {
  event(phase);
  await sleep(Number(control.delay_ms || 0));
  while ((control.hold || []).includes(phase) &&
         !fs.existsSync(path.join(home,'release',phase))) await sleep(100);
  if (control.fail_at === phase) { event('failure',{at_phase:phase}); process.exit(42); }
}
function records() {
  const result = cp.spawnSync('python3',[path.join(home,'tools/guest.py'),'records'],{encoding:'utf8'});
  if (result.status !== 0) throw Error(result.stderr);
  return JSON.parse(result.stdout);
}
async function work() {
  const record = records().find(r=>r.pane_id === pane);
  if (!record) { event('no-record'); return; }
  const controlPath = path.join(home,'control.json');
  const control = fs.existsSync(controlPath) ? JSON.parse(fs.readFileSync(controlPath)) : {};
  report('working');
  const skill = cp.spawnSync('ha',['skill','lane'],{encoding:'utf8'});
  if (skill.status !== 0) throw Error(skill.stderr);
  const brief = path.join(record.thread_dir,'brief.md');
  event('brief',{path:brief,sha256:crypto.createHash('sha256').update(fs.readFileSync(brief)).digest('hex')});
  await checkpoint(record.role === 'reviewer' ? 'mid-review' : 'working', control);
  const output = path.join(process.cwd(), 'scripted-'+record.id+'.txt');
  const git = argv => {
    const r = cp.spawnSync('git',argv,{encoding:'utf8'});
    if (r.status !== 0) throw Error(r.stderr);
  };
  let reportText = 'Scripted plumbing work; brief read, change committed.\n';
  if (record.role === 'reviewer') {
    const head = cp.execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
    reportText = `+++\nreview = "${record.review_id}"\nverdict = "REJECT"\ncandidate = "${head}"\n+++\nScripted reviewer exercises plumbing only; semantics are not established.\n`;
  } else {
    fs.writeFileSync(output, 'Scripted wall lane '+record.id+'\n');
    git(['add',path.basename(output)]);
    git(['commit','-m','Wall scripted lane '+record.id]);
  }
  fs.writeFileSync(path.join(record.thread_dir,'report.md'), reportText);
  await checkpoint('before-seal',control);
  // --after faults can also hit the real ha done helper; checkpoints are
  // honest boundaries, not claims that a held lane is inside the seal.
  const command = control.finish === 'waiting' ? ['waiting','Scripted wall input needed'] : ['done'];
  event('seal-start');
  const child = cp.spawn('ha',command,{stdio:'inherit',env:{...process.env,
    WALL_GIT_DELAY_SECONDS:String(Number(control.seal_delay_ms || 0)/1000)}});
  event('seal-helper',{helper_pid:child.pid});
  const code = await new Promise(resolve=>child.on('exit',resolve));
  event('seal-end',{code});
  if (code !== 0) throw Error('ha seal failed '+code);
  report('idle');
}
report('idle');
event('ready');
// Keep terminal paste boundaries intact until Enter. A single work turn starts
// on the first submitted brief, not for each newline within bracketed paste.
process.stdin.setRawMode?.(true);
process.stdin.resume();
let pasted = false, pending = '', busy = false;
process.stdin.on('data', async data => {
  pending += data.toString();
  if (pending.includes('\x1b[200~')) pasted = true;
  if (pending.includes('\x1b[201~')) pasted = false;
  if (pasted || !/[\r\n]/.test(pending)) return;
  pending = '';
  if (busy) return;
  if (args.includes('--wall-coordinator')) { event('coordinator-prompt'); return; }
  busy = true;
  try { await work(); } catch(e) { event('error',{message:e.message}); report('blocked'); }
});
