// Smoke test of the built JS API with a mocked native module (no device needed).
const Module = require('module');
const calls = [];
const native = {
  info: async () => JSON.stringify({ present: true, apiLevel: 1, version: '1.2.0', isFactory: false, slotDir: '/s' }),
  status: async () => JSON.stringify({ state: 'staged', release: '1.3.0' }),
  checkNow: () => calls.push('check'), applyNow: () => calls.push('apply'), restartNow: () => calls.push('restart'),
  markHealthy: () => calls.push('healthy'), listen: () => calls.push('listen'),
};
const orig = Module._load;
Module._load = function (req, ...rest) {
  if (req === 'react-native') {
    return { Platform: { OS: 'android' }, NativeModules: { AndySU: native },
             NativeEventEmitter: class { addListener() { return { remove() {} }; } } };
  }
  return orig.call(this, req, ...rest);
};
const a = require('../lib/index.js');
(async () => {
  const i = await a.info();
  if (!i.present || i.version !== '1.2.0') throw new Error('info');
  const s = await a.status();
  if (s.state !== 'staged' || s.release !== '1.3.0') throw new Error('status');
  a.checkNow(); a.applyNow(); a.restartNow(); a.markHealthy(); const off = a.onStatus(() => {}); off();
  if (calls.join(',') !== 'check,apply,restart,healthy,listen') throw new Error('calls ' + calls);
  console.log('react-native binding smoke test ok');
})().catch((e) => { console.error(e); process.exit(1); });
