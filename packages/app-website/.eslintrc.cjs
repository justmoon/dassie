module.exports = {
  root: true,
  extends: ["@dassie/eslint-config/next"],
  ignorePatterns: ["dist"],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: ["./tsconfig.json"],
  },
}
