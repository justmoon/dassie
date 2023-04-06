export const convertVanityNodeIdToFriendly = (vanityNodeId: string) => {
  const underscoreIndex = vanityNodeId.indexOf("_")
  return vanityNodeId.slice(
    0,
    underscoreIndex === -1 ? undefined : underscoreIndex
  )
}
