# Third-party notices

## OpenArchiver

The resumable recursive mail-folder traversal and one-shot expired per-folder delta-cursor recovery in the Microsoft Graph adapter were selectively adapted from the design of OpenArchiver's Microsoft Graph connectors.

- Project: OpenArchiver, https://github.com/LogicLabs-OU/OpenArchiver
- Pinned revision reviewed: `2082eba984ca771c23c2a7c60fc9284794e24b9b`
- Source files reviewed:
  - `packages/backend/src/services/ingestion-connectors/MicrosoftConnector.ts`
  - `packages/backend/src/services/ingestion-connectors/GraphMailboxConnector.ts`
  - `packages/backend/src/services/ingestion-connectors/helpers/retry.ts`
- License at the pinned revision: GNU Affero General Public License v3

The Station implementation is a Rust adaptation modified to remain delegated-user-only through `/me`, bounded and resumable across calls, preserve Graph tombstones, retain strict trusted-continuation validation, and return typed retry/cursor-expiry outcomes rather than performing archive-specific dropping or tenant-wide `/users` enumeration.

No Inbox Zero application code was used.
