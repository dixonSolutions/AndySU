// The web fallback and the registered plugin shape.
const Module = require('module');
let registered;
const orig = Module._load;
Module._load = function (req, ...rest) {
  if (req === '@capacitor/core') return { registerPlugin: (name, impl) => { registered = { name, impl }; return impl.web; } };
  return orig.call(this, req, ...rest);
};
const { AndySU } = require('../dist/index.js');
(async () => {
  if (registered.name !== 'AndySU') throw new Error('plugin name');
  if ((await AndySU.info()).present !== false) throw new Error('web info');
  if ((await AndySU.applyPolicy()).policy !== 'live') throw new Error('policy');
  console.log('capacitor binding smoke test ok');
})().catch((e) => { console.error(e); process.exit(1); });
