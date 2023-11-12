"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const recommended_js_1 = __importDefault(require("./configs/recommended.js"));
const no_floating_failures_js_1 = require("./rules/no-floating-failures.js");
const no_misused_failures_js_1 = require("./rules/no-misused-failures.js");
module.exports = {
    configs: {
        recommended: recommended_js_1.default,
    },
    rules: {
        "no-floating-failures": no_floating_failures_js_1.rule,
        "no-misused-failures": no_misused_failures_js_1.rule,
    },
};
