"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.default = {
    plugins: ["@dassie"],
    rules: {
        "@dassie/no-floating-failures": "error",
        "@dassie/no-misused-failures": "error",
        "@dassie/no-top-level-mutables": "error",
        "@dassie/no-top-level-side-effects": "error",
    },
};
