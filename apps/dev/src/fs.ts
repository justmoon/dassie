import { stat } from "node:fs/promises";

/** Whether `path` exists. */
export async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}
