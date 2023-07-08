module.exports = {
  root: true,
  extends: ["@dassie/eslint-config/next"],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: ["./tsconfig.json"],
  },
}
