module.exports = {
  parser: "@typescript-eslint/parser",
  extends: [
    "eslint:recommended",
    "plugin:@typescript-eslint/recommended",
    "plugin:@typescript-eslint/recommended-requiring-type-checking",
    "plugin:@typescript-eslint/strict",
    "plugin:unicorn/recommended",
    "plugin:react/recommended",
    "plugin:react/jsx-runtime",
    "plugin:react-hooks/recommended",
    "prettier",
  ],

  plugins: ["@typescript-eslint", "tsdoc", "react"],
  env: {
    es2021: true,
  },
  ignorePatterns: [
    "node_modules",
    "dist",
    "coverage",
    ".eslintrc.cjs",
    "next.config.js",
    "rollup.config.js",
    "vite.config.js",
    "vite.*.config.js",
  ],
  rules: {
    "no-console": [
      "warn",
      { allow: ["debug", "info", "warn", "error", "clear"] },
    ],
    "unicorn/no-null": "off",
    "unicorn/no-useless-undefined": "off",
    "tsdoc/syntax": "warn",
    "@typescript-eslint/no-non-null-assertion": "off",
    "@typescript-eslint/no-invalid-void-type": "off",
    "object-shorthand": ["warn", "properties"],
  },
  overrides: [
    {
      files: ["*.{,c,m}js{,x}"],
      rules: {
        "tsdoc/syntax": "off",
      },
    },
    {
      files: ["packages/app-cli/index.js"],
      env: {
        node: true,
      },
    },
    {
      files: ["next-env.d.ts"],
      rules: {
        "unicorn/prevent-abbreviations": "off",
      },
    },
  ],
  settings: {
    react: {
      version: "18.0.0",
    },
  },
}
