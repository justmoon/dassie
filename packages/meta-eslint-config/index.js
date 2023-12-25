module.exports = {
  parser: "@typescript-eslint/parser",
  extends: [
    "eslint:recommended",
    "plugin:@typescript-eslint/recommended",
    "plugin:@typescript-eslint/recommended-requiring-type-checking",
    "plugin:@typescript-eslint/strict",
    "plugin:import/recommended",
    "plugin:import/typescript",
    "plugin:unicorn/recommended",
    "plugin:react/recommended",
    "plugin:react/jsx-runtime",
    "plugin:react-hooks/recommended",
    "plugin:n/recommended-module",
    "plugin:eslint-comments/recommended",
    "plugin:@dassie/recommended",
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
    "@typescript-eslint/no-unused-vars": [
      "error",
      { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
    ],
    "object-shorthand": ["warn", "properties"],

    // Handled by TypeScript
    "import/namespace": "off",
    "import/default": "off",
    "import/no-unresolved": "off",
    "import/no-named-as-default-member": "off",

    "n/no-missing-import": "off",
    "n/no-unpublished-import": "off",
    "n/no-extraneous-import": "off",
    "n/shebang": "off",
    "n/no-process-exit": "off",
    "eslint-comments/disable-enable-pair": ["error", { allowWholeFile: true }],

    "@dassie/no-top-level-side-effects": [
      "error",
      {
        allowSimpleAssignments: true,
        allowInExportlessModules: true,
        allowedNodes: [
          // Some libraries require global function calls for configuration
          // - { enableMapSet } from 'immer'
          // - { allowErrorProps, registerClass } from 'superjson'
          "ExpressionStatement:has(CallExpression[callee.name=/^(enableMapSet|allowErrorProps|registerClass)$/])",

          // Allow inline tests via vitest
          'IfStatement:has([name="vitest"])',
        ],
      },
    ],
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
    {
      files: ["*.test.ts{,x}"],
      rules: {
        "unicorn/consistent-function-scoping": "off",
        "@dassie/no-top-level-side-effects": "off",
      },
    },
    {
      files: ["**/examples/**"],
      rules: {
        "@dassie/no-top-level-side-effects": "off",
      },
    },
    {
      files: ["**/bin/**"],
      rules: {
        "@dassie/no-top-level-side-effects": "off",
      },
    },
  ],
  settings: {
    react: {
      version: "18.0.0",
    },
    "import/resolver": "typescript",
  },
}
