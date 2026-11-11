module.exports = function(api) {
  const isProd = api.env('production') || process.env.NODE_ENV === 'production';

  return {
    presets: ['@react-native/babel-preset'],
    plugins: [
      // Source location only in non-production settings
      !isProd && './babel-plugin-function-source-location.js',
    ].filter(Boolean),
  };
};
