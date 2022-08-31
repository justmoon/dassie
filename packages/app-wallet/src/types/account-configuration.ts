export interface NullUplink {
  nodeId: string
  url: string
}

export interface NullAccountConfiguration {
  type: "null"
  uplinks: NullUplink[]
}

export type AccountConfiguration = NullAccountConfiguration
