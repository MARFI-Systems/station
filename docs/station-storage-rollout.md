# Station storage foundation rollout (bounded scope)

Status: **template only, not deployed, not validated against AWS.** This document
and `infra/station-storage/template.json` cover exactly one slice of the broader
storage migration: a private-by-default AWS foundation for five logical blob
roles plus supporting plumbing. No provisioning happened, no AWS credentials
were used, and no existing infrastructure was touched.

## Scope and ownership boundary

This work owns **only**:
- `infra/station-storage/**` (template + tests)
- `docs/station-storage-rollout.md` (this file)

Explicitly **not** touched or owned here:
- `crates/macro_aws_config/**` — owned by another workstream.
- Existing file consumers (`document_storage_service`, `static_file_service`,
  `email_service`, `document_upload_finalizer_handler`, etc.) — owned by
  another workstream. This template does not modify their code, task
  definitions, or runtime IAM roles.
- CI (`.github/workflows/station-storage-check.yml`,
  `.github/scripts/check-station-storage.sh`) — owned by the parent/CI worker.
  Nothing here changes that pipeline; it already runs offline (`SQLX_OFFLINE`,
  no AWS credentials, no live provider calls) and this template preserves that
  posture by never being invoked or deployed from it.
- The existing Pulumi stacks under `infra/stacks/**` (`static-file-service`,
  `document-storage`, `bulk-upload`, `email-service`, `cloud-storage-service`,
  etc.). Those remain the actual deployed infrastructure today. This
  CloudFormation template is a **parallel, inert artifact** — it is not wired
  into `pulumi up`, not imported by any stack, and does not supersede the
  Pulumi-managed resources.

## Why CloudFormation JSON instead of extending the existing Pulumi graph

The repo's real infra is Pulumi/TypeScript, and a decisively simpler path
would normally mean extending it. It was rejected here for three reasons:

1. **Blast radius.** The existing stacks (`static-file-service.ts`,
   `document-storage/index.ts`, etc.) are live, stack-referenced by other
   Pulumi projects (`cloud-storage-stack`, `document-storage-bucket-integrations`),
   and wired to a running ECS service, CloudFront distribution, and Lambda.
   Touching them pulls in the entire upstream dependency graph (VPC lookups,
   `StackReference`s, Doppler secrets, ECR images) that this task was
   explicitly told to avoid.
2. **Auditability.** A single, self-contained CloudFormation JSON file with no
   `npm install`, no Pulumi state, and no provider plugin is easier to read
   top-to-bottom and diff than a change buried inside a much larger
   TypeScript component tree.
3. **No accidental deployment.** Pulumi resources in this repo deploy the
   moment `pulumi up` runs in that stack directory. A standalone
   CloudFormation template requires an explicit, separate `aws cloudformation
   deploy` invocation with its own parameters file, which is a stronger
   guardrail against "it got deployed by accident during unrelated work."

The tradeoff: this template intentionally does **not** reuse existing bucket
names, the existing static-file table, or the existing notification queue. It
defines **new, empty-by-default parameters** for every name so a human
operator decides, later and separately, whether this ever replaces or runs
alongside the existing Pulumi-managed resources. Reconciling the two (e.g.
retiring the Pulumi bucket in favor of this one) is out of scope and would be
its own, carefully-sequenced migration with a real cutover plan.

## AWS S3 vs Backblaze B2

Danny authorized building proper Station storage and said B2 remains on the
table if it's cost effective. This template targets **AWS S3** as the active
choice for now because:

- **Native event semantics.** S3 → SQS `ObjectCreated` notifications, scoped
  IAM conditions on `aws:SourceArn`/`aws:SourceAccount`, and CloudFormation's
  first-class `AWS::S3::Bucket.NotificationConfiguration` are how the existing
  finalizer/search-upload/text-extractor pipeline already works
  (`infra/stacks/document-storage-bucket-integrations/index.ts`). B2's event
  notification story (webhooks via B2 Event Notifications, still newer/more
  limited than S3's) would require re-deriving that whole event chain.
  Migrating buckets between different topic-diffusion models is a real
  project, not a `Bucket.Provider` swap.
- **Existing IAM and consumer code already assumes S3.** `crates/documents`,
  `crates/call`, `crates/projects` all issue presigned S3 URLs
  (`s3_upload_url.rs`, `s3_upload_urls.rs`, `s3_recording_storage.rs`) through
  `macro_aws_config`. Since that crate is owned by a separate worker in this
  task, this template does not force a provider abstraction it can't verify.
- **B2 stays viable later.** B2 exposes an S3-compatible API for the object
  storage surface itself (buckets/objects/multipart), so the *S3 SDK calls*
  in the consumer crates would likely port with a custom endpoint. What does
  **not** port for free is the event/notification layer this template builds
  (S3 → SQS, EventBridge rules, scoped bucket-policy conditions). If B2
  becomes the cost-driven choice later, treat this template's SQS
  notification plumbing as the part that needs a redesign (e.g. polling,
  B2's own webhook mechanism, or a thin translation Lambda), not the bucket
  hardening posture (Block Public Access equivalent, encryption, lifecycle),
  which is provider-agnostic in spirit even though it's expressed here in
  CloudFormation/S3 terms.

This is a recommendation for the active/default path, not a decision lock-in.
Nothing in this template makes a future B2 migration harder than it would
otherwise be, since object keys are preserved as-is (see below) and no
tenant-prefix rewriting is introduced that a different provider would need to
unwind.

## What's in the template

Five logical blob roles, each its own bucket (exact names are parameters, not
generated):

| Bucket | Durable/versioned | Browser CORS (exact methods, evidence below) | Notifies SQS | Notes |
|---|---|---|---|---|
| Documents | Yes | `PUT` | Yes → `DocumentsQueue` | |
| Static | Yes | `GET, PUT, POST, DELETE, HEAD` | Yes → `StaticQueue` | Matches existing `static-file-service.ts` shape (bucket + metadata table) |
| Email attachments | Yes (explicit) | `PUT` | No | `temp/` prefix TTL kept, but is no longer the only recovery path (see below) |
| DOCX staging | No (documented) | `PUT` | No | Service-to-service pipeline output, but browser-uploaded input; whole-bucket TTL |
| Upload staging | No (documented) | `PUT` | No | Whole-bucket TTL |

**Correction from an earlier draft of this document:** it previously said
"documents is the only durable canonical store" to justify scoping backup to
it alone. That claim was wrong and has been removed — static assets, email
attachments, and document/sync state in the database all matter too. The
backup bucket below is now a generic destination, not a documents-only one.

Plus one **generic, encrypted Station backup destination bucket**,
conditionally created via `EnableBackupBucket` (**default `false`** — off
until a backup provider and copy/export mechanism are actually approved).
When enabled, it requires a non-empty `BackupBucketName`, enforced by a
template-level `Rules` section (`BackupBucketNameRequiredWhenEnabled`) so a
stack operation fails fast instead of silently deploying an unnamed/broken
bucket. This template does **not** decide which source(s) feed it, does not
wire any `AWS::S3::Bucket` replication configuration, copy job, or schedule,
and does not claim any bucket is "the" backed-up store — that selection is a
separate, later decision once a backup provider is chosen.

Access to the backup bucket is split into two distinct, unattached managed
policies, matching how backup/restore operations actually differ in
privilege:
- `BackupWriterAccessPolicy` — `s3:PutObject` plus the multipart-cleanup
  actions, and explicitly **no** `s3:DeleteObject`/`s3:DeleteObjectVersion`.
  A compromised or misbehaving backup-writer identity cannot destroy prior
  backups.
- `BackupRestoreAccessPolicy` — read-only (`s3:GetObject`,
  `s3:GetObjectVersion`, `s3:ListBucket`, `s3:ListBucketVersions`), intended
  for a separate restore identity/process.

Neither policy is referenced by any of the five app-facing access policies
(asserted by test), and neither is attached to any role by this template.

Every bucket gets, unconditionally:
- `PublicAccessBlockConfiguration` with all four flags `true`.
- `OwnershipControls: BucketOwnerEnforced` (ACLs disabled entirely).
- SSE-S3 (`AES256`), never SSE-KMS, to avoid per-request KMS charges.
- A bucket policy denying any request where `aws:SecureTransport` is `false`.
- `AbortIncompleteMultipartUpload` (parameterized days, default 7).
- `DeletionPolicy: Retain` / `UpdateReplacePolicy: Retain` — a stack
  delete/replace never silently deletes data.
- An explicit, parameterized, conservative lifecycle policy (see table above
  for durable vs. staging behavior). None of the defaults are aggressive;
  they're meant to be tuned per environment via the CloudFormation parameters,
  not hardcoded.

### Email attachments bucket: explicit versioning, not just a 1-day TTL

An earlier draft relied on the `temp/` prefix's 1-day expiration as the only
safety net for this bucket. That's not durable protection against accidental
overwrite or delete of an attachment outside `temp/`. The bucket now
explicitly enables `VersioningConfiguration: Enabled` plus a parameterized
noncurrent-version expiration (`EmailAttachmentsNoncurrentVersionExpirationDays`,
default 14 days) and an expired-delete-marker cleanup rule, in addition to
(not instead of) the existing `temp/` TTL.

### CORS: exact per-role methods, backed by code evidence

CORS is applied **only** to buckets with actual, cited browser-facing traffic,
and each bucket's `AllowedMethods` is the exact set that code evidence
supports — not a shared three-bucket default:

- **Documents — `PUT` only.** `crates/documents/src/outbound/s3_upload_url.rs`
  `put_document_storage_presigned_url` backs a presigned URL that
  `apps/web/src/lib/service-clients/service-storage/util/upload.ts` uploads to
  via `uploadToPresignedUrl()`, which does a `fetch(presignedUrl, { method:
  'PUT' })`. No fetch-based GET evidence was found (downloads/exports use
  browser navigation, which isn't CORS-gated), so GET was **not** added.
- **Static — `GET, PUT, POST, DELETE, HEAD`.** This mirrors the existing,
  already-deployed static-file-service bucket's own CORS rule verbatim
  (`infra/stacks/static-file-service/static-file-service.ts` `corsRules`).
  That's the strongest evidence available for this bucket's real surface.
- **Email attachments — `PUT` only.**
  `services/email_service/src/api/email/drafts/add_attachment.rs` generates a
  presigned PUT (`ctx.s3_client.put_presigned_url(&ctx.config.attachment_bucket,
  ...)`) returned to the client as `upload_url`;
  `apps/web/src/lib/queries/email/attachment.ts` uploads it via the same
  `uploadToPresignedUrl()` fetch-PUT helper as documents. **This directly
  contradicts an initial assumption that email attachments only need
  GET/HEAD** — real code shows a direct browser PUT, so PUT was added instead
  of GET/HEAD. Downloads go through
  `services/email_service/src/api/email/attachments/get.rs`'s presigned GET
  and `apps/web/src/features/email-message/attachment-action-adapter.ts`'s
  `window.open(dataUrl)` — a top-level navigation, which browsers do not
  subject to CORS — so GET/HEAD were not added without their own fetch
  evidence.
- **DOCX staging — `PUT` only.** `crates/documents/src/domain/service.rs`
  calls `put_docx_upload_presigned_url` for `FileType::Docx` uploads, and the
  resulting URL flows through the exact same generic `uploadWithPresignedUrl`
  → `uploadToPresignedUrl` PUT path in `upload.ts` used for every other
  document type. This bucket previously had no CORS configuration in this
  template at all; that was a gap given this evidence, now fixed.
- **Upload staging — `PUT` only.**
  `crates/projects/src/outbound/s3_upload_urls.rs`
  `put_upload_zip_staging_presigned_url` is returned by
  `crates/projects/src/inbound/axum_router/upload_folder.rs` for a direct
  client PUT of a folder-upload zip. This is narrower than an earlier draft's
  `GET, PUT, POST, HEAD` default, which had no code citation backing GET/POST.

All five CORS-enabled buckets are locked to the single `StationOriginUrl`
parameter, which must match `^https://...$` — plain HTTP or a bare hostname
is rejected at the parameter-validation level, before any deploy would even
reach the API. The parameter's constraint text intentionally does not
hardcode an example hostname (e.g. `app.station.example.com`); it only states
the scheme/format requirement, since a concrete example could be mistaken for
an expected literal value.

Object keys are preserved as-is everywhere. There is no generated
tenant-prefix rewriting logic in this template (this is asserted by an
automated test), matching how the existing `document-storage`,
`static-file-service`, and `bulk-upload` stacks already key objects.

### SQS notification path (documents + static only)

Both durable buckets notify SQS directly (`s3:ObjectCreated:*`), each with
its own DLQ and a `RedrivePolicy` (parameterized `MaxReceiveCount`, default
5). The main queues use SQS-managed SSE, a 20s long-poll receive wait, and
parameterized visibility timeout / retention. The queue resource policy that
lets S3 call `sqs:SendMessage` is scoped with both `aws:SourceArn` (the
specific bucket ARN) and `aws:SourceAccount` — not a bare `*` principal.

This intentionally mirrors, rather than duplicates, the existing
`document-storage-bucket-integrations` EventBridge pattern in the current
Pulumi stacks; it's a self-contained alternative wiring for this specific new
foundation, not a claim that it replaces the live EventBridge rules today.
The existing `document_upload_finalizer_handler` (classic S3 → SQS handler
service already in the repo) is compatible with this shape but is **not
wired up, deployed, or declared ready** by this template — that integration,
if desired, is explicitly left to the file-consumer workstream.

### CloudFormation dependency-cycle workaround (documented tradeoff)

S3 → SQS notification configuration has a well-known CloudFormation ordering
problem: the bucket's `NotificationConfiguration` needs the queue's policy to
already permit `SendMessage` from that bucket, but the natural way to write
that queue policy (`Fn::GetAtt Bucket.Arn` in a `Condition`) creates a
circular `Bucket → QueuePolicy → Bucket` dependency once you add the
necessary `DependsOn` from the bucket to the policy.

This template avoids the cycle by building the `aws:SourceArn` condition in
each queue policy from the **bucket name parameter** via `Fn::Sub`
(`arn:${AWS::Partition}:s3:::${BucketName}`) rather than `Fn::GetAtt` on the
bucket resource. Since bucket names here are user-supplied parameters (not
CloudFormation-generated), this is safe and doesn't reduce specificity — it's
the same ARN either way, just resolved from a different (a-cyclic) input.
`DocumentsBucket`/`StaticBucket` then declare an explicit `DependsOn` on
their respective `QueuePolicy` resource. The test suite verifies the overall
resource graph is acyclic by walking every `Ref`/`Fn::GetAtt`/`DependsOn`
edge and running a cycle check.

### IAM: least privilege, nothing attached

Every access policy in this template is a standalone `AWS::IAM::ManagedPolicy`
with **no attachment** to any role, user, group, or instance profile:

- `DocumentsAppAccessPolicy`, `StaticAppAccessPolicy`,
  `EmailAttachmentsAppAccessPolicy`, `DocxStagingAppAccessPolicy`,
  `UploadStagingAppAccessPolicy` — each scoped to exactly one bucket's ARN
  (plus, for static, the metadata table + its GSI). Each of these five also
  grants `s3:AbortMultipartUpload` and `s3:ListMultipartUploadParts` on the
  object resource and `s3:ListBucketMultipartUploads` on the bucket resource
  — matching the `AbortIncompleteMultipartUpload` lifecycle cleanup declared
  on every bucket, so the app identity that initiates a multipart upload can
  also list/abort its own in-flight parts, without any wildcard grant.
- `NotificationQueueConsumerPolicy` — scoped to the two main queues only
  (not their DLQs, which are meant for ops/redrive tooling, not app
  consumers).
- `BackupWriterAccessPolicy` / `BackupRestoreAccessPolicy` — scoped to the
  backup bucket only, deliberately separate from every app policy above and
  from each other (write-only-no-delete vs. read-only; see the backup section
  above).

No `AWS::IAM::Role`, `AWS::IAM::RolePolicy`, or
`AWS::IAM::RolePolicyAttachment` resource exists in this template (asserted
by a test) — this template cannot modify the existing ECS task role
(`static-file-service-role-${stack}` etc.) even by accident, because it
never references it. Whoever eventually wires these managed policies to a
real role does that outside this template, as a separate, explicit step.

### Static metadata table

`StaticMetadataTable` mirrors the existing Pulumi-managed table exactly on
schema: partition key `file_id` (String, matching
`services/static_file_service/src/service/dynamodb/model.rs`'s
`MetadataObject.file_id: String`), plus the `owner-index` GSI on `owner_id`
(String), matching `infra/stacks/static-file-service/static-file-service.ts`.
On top of that existing shape, this template adds:
- `PointInTimeRecoverySpecification.PointInTimeRecoveryEnabled: true`
- `SSESpecification.SSEEnabled: true` (AWS-owned key, no additional KMS cost
  by default — this is distinct from specifying `SSEType: KMS` with a
  customer/AWS-managed CMK, which does carry per-request charges)
- `BillingMode: PAY_PER_REQUEST` (already true in the existing table)
- `DeletionPolicy`/`UpdateReplacePolicy: Retain`

This is a new table definition parameterized by name — it does not import or
modify the live table. Reconciling the two remains a separate decision.

## What this template deliberately does NOT do

- **No deployment.** Nothing here runs `aws cloudformation deploy`,
  `create-stack`, or any AWS API call. No credentials were read or used.
- **No account IDs, ARNs with real account numbers, or secrets** appear
  anywhere in the template; all account/region references use
  `AWS::AccountId` / `AWS::Region` pseudo-parameters.
- **No new paid resources beyond what's unavoidable for the buckets/queues/
  table themselves** — no Lambda, no CloudFront, no KMS customer keys, no
  cross-region replication (unlike the existing `document-storage`
  Pulumi stack, which replicates to `us-west-1`; that's out of scope here and
  would be a real cost/complexity tradeoff to revisit deliberately).
- **No implication that the existing finalizer/consumer services are wired
  up, deployed, or ready.** They are referenced only in prose above as
  context for why the SQS shape looks the way it does.
- **No migration or backfill logic.** This is infrastructure-only; moving
  objects from any existing bucket into these new ones (or vice versa) is
  not addressed and would need its own plan with rollback and cutover
  windows.

## Validation performed vs. still required

**Performed (this task):**
- `node --test infra/station-storage/test/template.test.mjs` — 37 assertions,
  all passing, covering: public-access blocking, ownership controls, SSE-S3
  (never KMS), deny-non-TLS bucket policies, `Retain` deletion policies,
  versioning + noncurrent-version expiration on durable buckets (documents,
  static, email attachments, backup) with staging buckets' unversioned status
  asserted as intentional, multipart-abort on every bucket, bounded TTL on
  staging buckets, the email-attachments temp/-prefix-plus-noncurrent-version
  combination, CORS restricted to the single parameterized HTTPS origin with
  **exact evidence-backed per-bucket method sets** (not a shared default) on
  all five CORS-enabled buckets, a check that the origin parameter's
  constraint text carries no hardcoded example hostname, the backup bucket's
  default-disabled state, the `Rules`-section non-empty-name-when-enabled
  requirement, the writer/restore policy split (writer can `PutObject` but
  never `DeleteObject`/`DeleteObjectVersion`; restore is read-only), an
  absence-of-replication-configuration check, backup-policy isolation from
  every app-facing policy, absence of any IAM role/attachment resources,
  absence of wildcard-admin IAM statements, presence of
  `AbortMultipartUpload`/`ListMultipartUploadParts`/`ListBucketMultipartUploads`
  on every app access policy without a wildcard resource, SQS notification
  policy scoping (`aws:SourceArn`/`aws:SourceAccount`), DLQ/redrive wiring,
  SQS-managed encryption, SQS numeric parameters bounded to real AWS service
  limits, basic S3/SQS/DynamoDB naming-rule regexes on every name parameter,
  the DynamoDB key schema/GSI/billing/PITR/SSE properties, and a full
  dependency-cycle scan across every `Ref`/`Fn::GetAtt`/`DependsOn` edge in
  the template plus a dangling-reference scan.
- JSON structural validity (the template parses; re-serializes without loss).

**Still required, honestly, before this is "done":**
- **CloudFormation service-side validation.** `aws cloudformation validate-template`
  and a real `create-change-set` / `describe-change-set` against an actual
  AWS account were not run (no AWS calls were made, per the task's
  constraints). Static JSON correctness and internal consistency are not the
  same guarantee as CloudFormation's own schema/type/circular-reference
  validation — e.g., CloudFormation could still reject
  `AbortIncompleteMultipartUpload`-only lifecycle rules without other
  filters/prefixes in ways this offline check can't see, or flag a resource
  limit this review didn't anticipate.
- **A real end-to-end provider test.** Actually creating the stack in a
  sandbox account, confirming the S3 → SQS notification fires, confirming
  the deny-non-TLS policy doesn't accidentally break legitimate in-VPC
  traffic (some AWS service-to-service calls use TLS but through paths that
  interact differently with `aws:SecureTransport`), and confirming DynamoDB
  PITR/SSE settings apply as expected, are all still open.
- **Reconciliation with the live Pulumi stacks.** Whether this ever replaces,
  runs alongside, or is abandoned in favor of the existing
  `static-file-service`/`document-storage`/`bulk-upload`/`email-service`
  Pulumi resources is an explicit open decision, not resolved here.
- **B2 cost/feasibility analysis.** This document states the rationale for
  defaulting to S3 now; it does not include an actual B2 pricing/latency
  comparison. That remains a follow-up if B2 is seriously considered.
- **No account-specific findings are recorded here** — this review is scoped
  to the template's own construction, not a live-account security audit.

## Files

- `infra/station-storage/template.json` — the CloudFormation template.
- `infra/station-storage/test/template.test.mjs` — Node built-in
  (`node:test` + `node:assert`) regression tests, run with
  `node --test infra/station-storage/test/template.test.mjs`. No
  dependencies to install; no AWS SDK; no network calls.
