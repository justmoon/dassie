import { unionTypeParts } from "tsutils"
import { Type, TypeChecker } from "typescript"

export const FAILURE_UNIQUE_KEY = "dassie.failure"

export const isSometimesFailureLike = (
  checker: TypeChecker,
  type: Type,
): boolean => {
  for (const typePart of unionTypeParts(checker.getApparentType(type))) {
    if (typePart.getProperty(FAILURE_UNIQUE_KEY) !== undefined) {
      return true
    }
  }
  return false
}

export const isAlwaysFailureLike = (
  checker: TypeChecker,
  type: Type,
): boolean => {
  for (const typePart of unionTypeParts(checker.getApparentType(type))) {
    if (typePart.getProperty(FAILURE_UNIQUE_KEY) === undefined) {
      return false
    }
  }
  return true
}

export const returnsFailureLike = (
  checker: TypeChecker,
  type: Type,
): boolean => {
  const apparentType = checker.getApparentType(type)
  for (const signature of apparentType.getCallSignatures()) {
    const returnType = signature.getReturnType()
    if (isSometimesFailureLike(checker, returnType)) {
      return true
    }
  }

  return false
}
