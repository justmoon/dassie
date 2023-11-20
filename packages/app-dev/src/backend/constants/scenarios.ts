import { NodeSettings, Scenario } from "../stores/scenario"

export const builtinScenarios = {
  sixAutopeeringStubNodes: {
    environment: {},
    nodes: Array.from<NodeSettings>({ length: 6 }).fill({}),
  },
} as const satisfies Record<string, Scenario>
