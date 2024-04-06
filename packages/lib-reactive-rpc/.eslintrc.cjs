module.exports = {
  root: true,
  extends: ["@dassie"],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: ["./tsconfig.json"],
  },
}
