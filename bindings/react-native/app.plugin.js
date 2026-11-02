// Expo config plugin. AndySU needs nothing in the app's own manifest: the frozen host is produced
// from the finished APK by `andysu split --transparent` (or the Gradle plugin). The plugin records
// the andysu.toml location in the Android app's gradle.properties so build scripts can find it.
const { withGradleProperties } = require('@expo/config-plugins');

module.exports = function withAndySU(config, props = {}) {
  return withGradleProperties(config, (c) => {
    const key = 'andysu.toml';
    c.modResults = c.modResults.filter((p) => !(p.type === 'property' && p.key === key));
    c.modResults.push({ type: 'property', key, value: props.toml || '../andysu.toml' });
    return c;
  });
};
