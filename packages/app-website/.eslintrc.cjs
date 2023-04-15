module.exports = {
  root: true,
  extends: ["@dassie", "next"],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: ["./tsconfig.json"],
  },
}
