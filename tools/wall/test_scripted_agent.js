// Regression: a reviewer must load its own skill before reaching mid-review.
const assert = require('assert/strict');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const {EventEmitter} = require('events');
const source = fs.readFileSync(path.join(__dirname, 'scripted-agent.js'), 'utf8');

async function turn(role) {
  const events = [], calls = [], files = new Map();
  const stdin = new EventEmitter();
  stdin.setRawMode = stdin.resume = () => {};
  const home = '/home/wall';
  const worktree = home + '/repo/.worktrees/t-0001';
  const record = {id:'t-0001', pane_id:'w1:p3', role, review_id:'review-1',
    thread_dir:worktree + '/.herdr-project/wall-t-0001'};
  const childProcess = {
    spawnSync(command, args) {
      calls.push([command, ...args]);
      if (command === 'python3') return {status:0, stdout:JSON.stringify([record])};
      if (command === 'ha' && args[0] === 'skill' && args[1] !== role)
        return {status:1, stderr:`bootstrap_mismatch: run skill ${role}`};
      return {status:0, stdout:''};
    },
    execFileSync: () => 'a'.repeat(40) + '\n',
    spawn(command, args) {
      calls.push([command, ...args]);
      const child = new EventEmitter();
      child.pid = 123;
      setImmediate(() => child.emit('exit', 0));
      return child;
    },
  };
  vm.runInNewContext(source, {
    require(name) {
      if (name === 'child_process') return childProcess;
      if (name === 'fs') return {
        mkdirSync() {}, existsSync: () => false,
        readFileSync: () => Buffer.from('frozen brief'),
        writeFileSync: (file, text) => files.set(file, text),
        appendFileSync: (file, text) => events.push(JSON.parse(text)),
      };
      return require(name);
    },
    process:{argv:['node', 'scripted-agent.js', '--wall-lane'],
      env:{HOME:home, HERDR_PANE_ID:record.pane_id}, stdin, cwd:() => worktree},
    console:{log() {}}, setTimeout,
  });
  stdin.emit('data', Buffer.from('brief\r'));
  for (let attempt = 0; attempt < 100; attempt++) {
    if (events.some(row => row.phase === 'seal-end' || row.phase === 'error')) break;
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  assert(!events.some(row => row.phase === 'error'), JSON.stringify(events));
  assert(calls.some(call => JSON.stringify(call) === JSON.stringify(['ha','skill',role])));
  assert(events.some(row => row.phase === (role === 'reviewer' ? 'mid-review' : 'working')));
  assert(events.some(row => row.phase === 'seal-end' && row.code === 0));
  assert(calls.some(call => JSON.stringify(call) === JSON.stringify(['ha','done'])));
  const report = files.get(record.thread_dir + '/report.md');
  if (role === 'reviewer') {
    assert(report.includes('verdict = "REJECT"'));
    assert(!calls.some(call => call[0] === 'git' && call[1] === 'commit'));
  } else {
    assert(calls.some(call => call[0] === 'git' && call[1] === 'commit'));
  }
}

(async () => {
  await turn('lane');
  await turn('reviewer');
  console.log('scripted lane/reviewer skill and sealing regression: PASS');
})().catch(error => { console.error(error); process.exitCode = 1; });
