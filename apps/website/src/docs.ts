/**
 * The docs at dassie.land/docs/: their sections and pages, in the order
 * the sidebar lists them. This is the only list of them. The build makes
 * a page of each entry (vite.config.ts), puts the docs layout around it
 * and writes each section's index from it (src/docs-layout.ts), and
 * `docs_test.ts` checks that every entry has its file and every file its
 * entry.
 *
 * A page's one-line description is its `<meta name="description">`, read
 * from its file at build time, so it is written once. A page outside
 * /docs/ (the architecture page, the STREAM toy, the brand page) is listed
 * for the sidebar and stays where it is.
 *
 * @module
 */

/** A page the sidebar lists. */
export interface DocPage {
  path: string;
  /** Its name in the sidebar, its `<h1>`, and its `<title>` before
   * " · Dassie". */
  title: string;
}

/** A section of the docs, with an index page at its path. */
export interface Section {
  id: string;
  name: string;
  path: string;
  /** What the section holds, in a sentence: on its index and the docs
   * home. */
  blurb: string;
  pages: readonly DocPage[];
}

/** The docs home. */
export const DOCS = "/docs/";

/** The search page, which every search box submits to. */
export const SEARCH = "/docs/search/";

/** Pages that moved, from their old path to their new one. The load
 * balancer redirects the old path, with and without its slash
 * (scripts/website/provision.sh). */
export const MOVED: readonly { from: string; to: string }[] = [
  { from: "/guides/quick-start/", to: "/docs/start/quick-start/" },
  { from: "/guides/nixos/", to: "/docs/node/nixos/" },
];

/** The sections, in sidebar order. */
export const SECTIONS: readonly Section[] = [
  {
    id: "start",
    name: "Start here",
    path: "/docs/start/",
    blurb:
      "What Dassie is, which setup fits you, a first node and a first payment.",
    pages: [
      { path: "/docs/start/what-is-dassie/", title: "What is Dassie?" },
      { path: "/docs/start/pick-your-setup/", title: "Pick your setup" },
      { path: "/docs/start/quick-start/", title: "Quick Start" },
      { path: "/docs/start/first-payment/", title: "Your first payment" },
      { path: "/docs/start/early-days/", title: "Early days" },
      { path: "/docs/start/glossary/", title: "Glossary" },
    ],
  },
  {
    id: "concepts",
    name: "Concepts",
    path: "/docs/concepts/",
    blurb:
      "How Dassie works, in plain words. Each page links to the spec for the full reasons.",
    pages: [
      { path: "/docs/concepts/payments/", title: "How a payment travels" },
      { path: "/docs/concepts/hubs/", title: "Settlement hubs" },
      { path: "/docs/concepts/phrase/", title: "Your recovery phrase" },
      { path: "/docs/concepts/ledgers/", title: "Ledgers, books and realms" },
      {
        path: "/docs/concepts/core-and-periphery/",
        title: "Core and periphery",
      },
      { path: "/docs/concepts/names/", title: "Names and addresses" },
      { path: "/docs/concepts/peers/", title: "Peers and holds" },
      { path: "/docs/concepts/network/", title: "Joining the network" },
      { path: "/docs/concepts/rates/", title: "Exchange rates" },
      { path: "/docs/concepts/versions/", title: "Protocol versions" },
      { path: "/docs/concepts/releases/", title: "Releases you can trust" },
    ],
  },
  {
    id: "wallet",
    name: "Use your wallet",
    path: "/docs/wallet/",
    blurb:
      "The owner's tasks in the web wallet, one page each, in the order a new owner meets them.",
    pages: [
      { path: "/docs/wallet/setup/", title: "Set up your wallet" },
      { path: "/docs/wallet/backup/", title: "Back up your phrase" },
      { path: "/docs/wallet/hubs/", title: "Your hubs" },
      { path: "/docs/wallet/ledgers/", title: "Add and fund a ledger" },
      { path: "/docs/wallet/receive/", title: "Receive money" },
      { path: "/docs/wallet/send/", title: "Send money" },
      { path: "/docs/wallet/names/", title: "Choose a name" },
      {
        path: "/docs/wallet/peg-guard/",
        title: "Dollar tokens and the peg guard",
      },
      { path: "/docs/wallet/activity/", title: "Activity and assets" },
      { path: "/docs/wallet/grants/", title: "Connect apps" },
      { path: "/docs/wallet/network/", title: "Network and peers" },
      { path: "/docs/wallet/api-keys/", title: "API keys" },
      { path: "/docs/wallet/rates/", title: "Exchange-rate settings" },
      { path: "/docs/wallet/devices/", title: "Paired devices" },
    ],
  },
  {
    id: "desktop",
    name: "Desktop app",
    path: "/docs/desktop/",
    blurb: "Your node in a window on your own computer: no server, no domain.",
    pages: [
      { path: "/docs/desktop/install/", title: "Install the desktop app" },
      { path: "/docs/desktop/reach/", title: "How others reach you" },
      { path: "/docs/desktop/lifecycle/", title: "Tray, login and quitting" },
      { path: "/docs/desktop/platforms/", title: "Other platforms" },
    ],
  },
  {
    id: "node",
    name: "Run a node",
    path: "/docs/node/",
    blurb:
      "For a server's operator: install, configure, watch, update, back up.",
    pages: [
      { path: "/docs/node/requirements/", title: "Requirements" },
      { path: "/docs/node/nixos/", title: "Dassie on NixOS" },
      { path: "/docs/node/headless/", title: "Headless setup" },
      { path: "/docs/node/configure/", title: "Configure your node" },
      { path: "/docs/node/status/", title: "Check on your node" },
      { path: "/docs/node/troubleshooting/", title: "Troubleshooting" },
      { path: "/docs/node/updates/", title: "Updates and rollback" },
      { path: "/docs/node/uninstall/", title: "Uninstall" },
      { path: "/docs/node/certificates/", title: "Certificates" },
      { path: "/docs/node/backup/", title: "Back up and move a node" },
      { path: "/docs/node/security/", title: "Security for operators" },
      { path: "/docs/node/periphery/", title: "A server without a public IP" },
    ],
  },
  {
    id: "services",
    name: "Run network services",
    path: "/docs/services/",
    blurb:
      "Roles beyond a node: hub validators and founders, market makers, front doors.",
    pages: [
      {
        path: "/docs/services/hub-validator/",
        title: "Validate a settlement hub",
      },
      { path: "/docs/services/found-a-hub/", title: "Found a hub" },
      { path: "/docs/services/market-maker/", title: "Run a market maker" },
      { path: "/docs/services/front-door/", title: "Run a front door" },
    ],
  },
  {
    id: "build",
    name: "Build on Dassie",
    path: "/docs/build/",
    blurb:
      "For app developers: pay and get paid through a node with standard Interledger tools.",
    pages: [
      { path: "/docs/build/overview/", title: "Integration overview" },
      {
        path: "/docs/build/open-payments/",
        title: "Accept payments with Open Payments",
      },
      {
        path: "/docs/build/signed-responses/",
        title: "Signed responses and names",
      },
      { path: "/docs/build/grants/", title: "Grants and limits" },
      { path: "/docs/build/btp/", title: "Connect an ILP client over BTP" },
      { path: "/docs/build/stream/", title: "STREAM on Dassie" },
      { path: "/docs/build/rate-feed/", title: "Read a rate feed" },
      { path: "/docs/build/errors/", title: "Error codes" },
      { path: "/docs/build/dev-network/", title: "Local dev network" },
      { path: "/docs/build/owner-apps/", title: "Owner apps" },
    ],
  },
  {
    id: "reference",
    name: "Reference",
    path: "/docs/reference/",
    blurb:
      "Facts to look up: commands, variables, files, schemes and releases, checked against the code.",
    pages: [
      { path: "/docs/reference/cli/", title: "The dassie command" },
      { path: "/docs/reference/environment/", title: "Environment variables" },
      {
        path: "/docs/reference/files-and-ports/",
        title: "Files, ports and sockets",
      },
      { path: "/docs/reference/mirror/", title: "Release mirror and key" },
      { path: "/docs/reference/releases/", title: "Release notes" },
      { path: "/docs/reference/schemes/", title: "Schemes and hubs" },
      { path: "/docs/reference/domains/", title: "Public domains" },
      { path: "/docs/reference/proof-servers/", title: "Proof servers" },
      { path: "/docs/reference/nixos-options/", title: "NixOS module options" },
      {
        path: "/docs/reference/open-payments/",
        title: "Open Payments endpoints",
      },
      { path: "/docs/reference/feed-format/", title: "Rate feed wire format" },
    ],
  },
  {
    id: "project",
    name: "Project",
    path: "/docs/project/",
    blurb: "How Dassie is built and looks, and where to get help.",
    pages: [
      { path: "/architecture/", title: "Architecture" },
      { path: "/tools/stream-toy/", title: "STREAM Toy" },
      { path: "/brand/", title: "Brand: Kopje" },
      { path: "/docs/project/help/", title: "Get help" },
      { path: "/docs/project/security/", title: "Report a security issue" },
      { path: "/docs/project/spec/", title: "Specification" },
      { path: "/docs/project/decisions/", title: "Design decisions" },
      { path: "/docs/project/roadmap/", title: "What's next" },
      { path: "/docs/project/contributing/", title: "Contributing" },
    ],
  },
];

/** Every page the sidebar lists, in its order. */
export const SIDEBAR: readonly DocPage[] = SECTIONS.flatMap((section) =>
  section.pages
);

/** Whether `path` is under /docs/. */
export function inDocs(path: string): boolean {
  return path.startsWith(DOCS);
}

/** The section a page under /docs/ belongs to, by its path. */
export function sectionOf(path: string): Section | undefined {
  return SECTIONS.find((section) =>
    path.startsWith(section.path) ||
    section.pages.some((page) => page.path === path)
  );
}

/** Every page the build makes under /docs/: the home, the search page,
 * each section's index and each page of a section that lives there. */
export function docsPaths(): string[] {
  return [
    DOCS,
    SEARCH,
    ...SECTIONS.flatMap((section) => [
      section.path,
      ...section.pages.map((page) => page.path).filter(inDocs),
    ]),
  ];
}
