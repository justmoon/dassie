import update from "@pnpm/meta-updater"

export async function runMetaUpdater() {
  await update({ test: true })
}
