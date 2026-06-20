// Expo's Babel preset drives both the Metro bundler and the jest-expo transform
// (Task 508). `babel-preset-expo` pulls in expo-router's plugin automatically.
module.exports = function (api) {
  api.cache(true);
  return {
    presets: ["babel-preset-expo"],
  };
};
