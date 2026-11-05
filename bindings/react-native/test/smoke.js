// Smoke test of the built JS API with a mocked native module (no device needed).
const Module = require('module');
const calls = [];
const native = {
  info: async () => JSON.stringify({ present: true, apiLevel: 1, version: '1.2.0', isFactory: false, slotDir: '/s' }),
  status: async () => JSON.stringify({ state: 'staged', release: '1.3.0' }),
  checkNow: () => calls.push('check'), applyNow: () => calls.push('apply'), restartNow: () => calls.push('restart'),
  setUpdateMode: (m) => calls.push('mode:' + m), updateMode: async () => modeValue,
  markHealthy: () => calls.push('healthy'), listen: () => calls.push('listen'),
};
let modeValue = 'download_only';
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
  a.setUpdateMode('check_only'); a.setUpdateMode(null);
  if (calls.slice(-2).join(',') !== 'mode:check_only,mode:null') throw new Error('setUpdateMode ' + calls);
  if ((await a.updateMode()) !== 'download_only') throw new Error('updateMode');
  modeValue = ''; // the bridge answers "" on a host older than API level 3
  if ((await a.updateMode()) !== null) throw new Error('updateMode on old host');
  console.log('react-native binding smoke test ok');
})().catch((e) => { console.error(e); process.exit(1); });
