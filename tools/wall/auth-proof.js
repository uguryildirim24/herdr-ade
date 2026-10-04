// Sandbox-only regression fixture, never a provider login or model call.
const { FileAuthStorageBackend } = await import(
  `${process.env.HOME}/pi-package/node_modules/@earendil-works/pi-coding-agent/dist/core/auth-storage.js`);
const backend = new FileAuthStorageBackend(`${process.env.HOME}/.herdr-ade/pi/agent/auth.json`);
const key = 'wall-isolation-proof';
backend.withLock(current => {
  const value = JSON.parse(current);
  switch (process.argv[2]) {
    case 'add':
      if (value[key]) throw Error('proof fixture already present');
      value[key] = { type: 'api_key', key: 'wall-reset-sentinel-not-a-credential' };
      break;
    case 'check':
      if (value[key]?.key !== 'wall-reset-sentinel-not-a-credential') throw Error('shared login lost');
      return { result: true };
    case 'remove':
      delete value[key];
      break;
    default: throw Error('expected add/check/remove');
  }
  return { result: true, next: JSON.stringify(value, null, 2) + '\n' };
});
console.log(`Shared auth backend ${process.argv[2]}: PASS`);
