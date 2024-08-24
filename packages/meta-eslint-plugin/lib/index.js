"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const recommended_js_1 = __importDefault(require("./configs/recommended.js"));
const no_floating_failures_js_1 = require("./rules/no-floating-failures.js");
const no_misused_failures_js_1 = require("./rules/no-misused-failures.js");
const no_top_level_mutables_js_1 = require("./rules/no-top-level-mutables.js");
const no_top_level_side_effects_js_1 = require("./rules/no-top-level-side-effects.js");
const plugin = {
    rules: {
        "no-floating-failures": no_floating_failures_js_1.rule,
        "no-misused-failures": no_misused_failures_js_1.rule,
        "no-top-level-mutables": no_top_level_mutables_js_1.rule,
        "no-top-level-side-effects": no_top_level_side_effects_js_1.rule,
    },
};
module.exports = {
    ...plugin,
    configs: {
        recommended: { plugins: { "@dassie": plugin }, ...recommended_js_1.default },
    },
};
