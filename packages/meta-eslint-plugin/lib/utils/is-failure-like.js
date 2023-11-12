"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.returnsFailureLike = exports.isAlwaysFailureLike = exports.isSometimesFailureLike = exports.FAILURE_UNIQUE_KEY = void 0;
const tsutils_1 = require("tsutils");
exports.FAILURE_UNIQUE_KEY = "dassie.failure";
const isSometimesFailureLike = (checker, type) => {
    for (const typePart of (0, tsutils_1.unionTypeParts)(checker.getApparentType(type))) {
        if (typePart.getProperty(exports.FAILURE_UNIQUE_KEY) !== undefined) {
            return true;
        }
    }
    return false;
};
exports.isSometimesFailureLike = isSometimesFailureLike;
const isAlwaysFailureLike = (checker, type) => {
    for (const typePart of (0, tsutils_1.unionTypeParts)(checker.getApparentType(type))) {
        if (typePart.getProperty(exports.FAILURE_UNIQUE_KEY) === undefined) {
            return false;
        }
    }
    return true;
};
exports.isAlwaysFailureLike = isAlwaysFailureLike;
const returnsFailureLike = (checker, type) => {
    const apparentType = checker.getApparentType(type);
    for (const signature of apparentType.getCallSignatures()) {
        const returnType = signature.getReturnType();
        if ((0, exports.isSometimesFailureLike)(checker, returnType)) {
            return true;
        }
    }
    return false;
};
exports.returnsFailureLike = returnsFailureLike;
