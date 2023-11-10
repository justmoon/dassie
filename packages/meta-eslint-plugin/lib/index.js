"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const recommended_js_1 = __importDefault(require("./configs/recommended.js"));
const must_handle_failure_js_1 = __importDefault(require("./rules/must-handle-failure.js"));
module.exports = {
    configs: {
        recommended: recommended_js_1.default,
    },
    rules: {
        "must-handle-failure": must_handle_failure_js_1.default,
    },
};
