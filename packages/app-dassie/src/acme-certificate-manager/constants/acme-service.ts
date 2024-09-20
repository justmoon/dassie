import { directory as acmeDirectory } from "acme-client"

export const ACME_DIRECTORY_URL =
  import.meta.env.DEV ?
    acmeDirectory.letsencrypt.staging
  : acmeDirectory.letsencrypt.production
