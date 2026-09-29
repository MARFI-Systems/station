// Node built-in test runner + assert. No external dependencies, no AWS calls.
// Run with: node --test infra/station-storage/test/template.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const templatePath = path.join(__dirname, '..', 'template.json');
const raw = readFileSync(templatePath, 'utf8');

test('template.json parses as valid JSON', () => {
  assert.doesNotThrow(() => JSON.parse(raw));
});

const tpl = JSON.parse(raw);
const resources = tpl.Resources;
const bucketResourceNames = Object.entries(resources)
  .filter(([, r]) => r.Type === 'AWS::S3::Bucket')
  .map(([name]) => name);

// ---------------------------------------------------------------------------
// Reference / dependency graph helpers
// ---------------------------------------------------------------------------

function collectRefs(node, ids) {
  if (Array.isArray(node)) {
    node.forEach((n) => collectRefs(n, ids));
    return;
  }
  if (node && typeof node === 'object') {
    for (const [key, value] of Object.entries(node)) {
      if (key === 'Ref' && typeof value === 'string') {
        ids.add(value);
        continue;
      }
      if (key === 'Fn::GetAtt') {
        const target = Array.isArray(value) ? value[0] : String(value).split('.')[0];
        if (typeof target === 'string') ids.add(target);
        continue;
      }
      collectRefs(value, ids);
    }
  }
}

function buildDependencyGraph(resources) {
  const graph = new Map();
  for (const [name, def] of Object.entries(resources)) {
    const ids = new Set();
    collectRefs(def.Properties, ids);
    if (Array.isArray(def.DependsOn)) def.DependsOn.forEach((d) => ids.add(d));
    else if (typeof def.DependsOn === 'string') ids.add(def.DependsOn);
    const resourceIds = [...ids].filter((id) => resources[id] && id !== name);
    graph.set(name, resourceIds);
  }
  return graph;
}

function findCycle(graph) {
  const WHITE = 0, GRAY = 1, BLACK = 2;
  const color = new Map([...graph.keys()].map((k) => [k, WHITE]));
  const stack = [];

  function visit(node) {
    color.set(node, GRAY);
    stack.push(node);
    for (const dep of graph.get(node) ?? []) {
      if (color.get(dep) === GRAY) {
        return [...stack.slice(stack.indexOf(dep)), dep];
      }
      if (color.get(dep) === WHITE) {
        const cyc = visit(dep);
        if (cyc) return cyc;
      }
    }
    stack.pop();
    color.set(node, BLACK);
    return null;
  }

  for (const node of graph.keys()) {
    if (color.get(node) === WHITE) {
      const cyc = visit(node);
      if (cyc) return cyc;
    }
  }
  return null;
}

test('CloudFormation resource reference graph has no dependency cycles', () => {
  const graph = buildDependencyGraph(resources);
  const cycle = findCycle(graph);
  assert.equal(cycle, null, `Found a dependency cycle: ${cycle ? cycle.join(' -> ') : ''}`);
});

test('every Ref/GetAtt/DependsOn target resolves to a declared resource, parameter, or pseudo-parameter', () => {
  const pseudo = new Set([
    'AWS::AccountId', 'AWS::Region', 'AWS::Partition', 'AWS::StackName',
    'AWS::StackId', 'AWS::URLSuffix', 'AWS::NoValue',
  ]);
  const declared = new Set([
    ...Object.keys(resources),
    ...Object.keys(tpl.Parameters ?? {}),
    ...pseudo,
  ]);
  const ids = new Set();
  collectRefs(tpl.Resources, ids);
  collectRefs(tpl.Outputs, ids);
  const dangling = [...ids].filter((id) => !declared.has(id));
  assert.deepEqual(dangling, [], `Dangling references found: ${dangling.join(', ')}`);
});

// ---------------------------------------------------------------------------
// Private-by-default bucket posture
// ---------------------------------------------------------------------------

test('every bucket blocks all public access', () => {
  for (const name of bucketResourceNames) {
    const pab = resources[name].Properties.PublicAccessBlockConfiguration;
    assert.ok(pab, `${name} is missing PublicAccessBlockConfiguration`);
    for (const flag of ['BlockPublicAcls', 'BlockPublicPolicy', 'IgnorePublicAcls', 'RestrictPublicBuckets']) {
      assert.equal(pab[flag], true, `${name}.${flag} must be true`);
    }
  }
});

test('every bucket enforces bucket-owner-enforced ownership (ACLs disabled)', () => {
  for (const name of bucketResourceNames) {
    const oc = resources[name].Properties.OwnershipControls;
    assert.ok(oc, `${name} is missing OwnershipControls`);
    assert.ok(
      oc.Rules.some((r) => r.ObjectOwnership === 'BucketOwnerEnforced'),
      `${name} must set ObjectOwnership: BucketOwnerEnforced`
    );
  }
});

test('every bucket uses SSE-S3 (AES256) by default, never SSE-KMS', () => {
  for (const name of bucketResourceNames) {
    const enc = resources[name].Properties.BucketEncryption;
    assert.ok(enc, `${name} is missing BucketEncryption`);
    for (const rule of enc.ServerSideEncryptionConfiguration) {
      assert.equal(rule.ServerSideEncryptionByDefault.SSEAlgorithm, 'AES256', `${name} must use AES256, not KMS, to avoid per-request KMS cost`);
      assert.equal(rule.ServerSideEncryptionByDefault.KMSMasterKeyId, undefined, `${name} must not reference a KMS key`);
    }
  }
});

test('every bucket denies non-TLS requests via its bucket policy', () => {
  for (const name of bucketResourceNames) {
    const policyName = `${name}Policy`;
    const policy = resources[policyName];
    assert.ok(policy, `${name} is missing a companion ${policyName} resource`);
    const statements = policy.Properties.PolicyDocument.Statement;
    const denyTls = statements.find((s) => s.Sid === 'DenyNonTLS');
    assert.ok(denyTls, `${policyName} must include a DenyNonTLS statement`);
    assert.equal(denyTls.Effect, 'Deny');
    assert.equal(denyTls.Condition?.Bool?.['aws:SecureTransport'], 'false');
  }
});

test('every bucket has a Retain deletion policy (no accidental data loss on stack changes)', () => {
  for (const name of bucketResourceNames) {
    const res = resources[name];
    assert.equal(res.DeletionPolicy, 'Retain', `${name}.DeletionPolicy must be Retain`);
    assert.equal(res.UpdateReplacePolicy, 'Retain', `${name}.UpdateReplacePolicy must be Retain`);
  }
});

// Durable/versioned set now includes email attachments (explicit versioning added per review --
// the temp/ prefix TTL must not be the only recovery mechanism for accidental overwrite/delete).
// Staging buckets (DOCX staging, upload staging) are deliberately excluded: they are transient,
// single-purpose, short-TTL buckets by design, documented in docs/station-storage-rollout.md.
const versionedBuckets = ['DocumentsBucket', 'StaticBucket', 'EmailAttachmentsBucket', 'BackupBucket'];
const intentionallyUnversionedBuckets = ['DocxStagingBucket', 'UploadStagingBucket'];

test('durable buckets (documents, static, email attachments, backup) are versioned with an explicit noncurrent-version expiration rule', () => {
  for (const name of versionedBuckets) {
    const props = resources[name].Properties;
    assert.equal(props.VersioningConfiguration?.Status, 'Enabled', `${name} must be versioned`);
    const rules = props.LifecycleConfiguration?.Rules ?? [];
    const noncurrentRule = rules.find((r) => r.NoncurrentVersionExpiration);
    assert.ok(noncurrentRule, `${name} must declare a noncurrent-version expiration lifecycle rule`);
    assert.ok(noncurrentRule.NoncurrentVersionExpiration.NoncurrentDays, `${name} noncurrent expiration must reference an explicit day count`);
  }
});

test('staging buckets are intentionally unversioned (transient, short-TTL, no version history value)', () => {
  for (const name of intentionallyUnversionedBuckets) {
    const props = resources[name].Properties;
    assert.equal(props.VersioningConfiguration, undefined, `${name} should not declare versioning`);
  }
});

test('every bucket has an AbortIncompleteMultipartUpload lifecycle rule', () => {
  for (const name of bucketResourceNames) {
    const rules = resources[name].Properties.LifecycleConfiguration?.Rules ?? [];
    const abortRule = rules.find((r) => r.AbortIncompleteMultipartUpload);
    assert.ok(abortRule, `${name} must declare AbortIncompleteMultipartUpload`);
  }
});

test('staging buckets (docx-staging, upload-staging) declare a bounded object expiration', () => {
  for (const name of intentionallyUnversionedBuckets) {
    const rules = resources[name].Properties.LifecycleConfiguration.Rules;
    const expiry = rules.find((r) => r.ExpirationInDays);
    assert.ok(expiry, `${name} must expire objects after a bounded number of days`);
  }
});

test('email attachments bucket keeps its temp/ prefix TTL in addition to (not instead of) noncurrent-version expiration', () => {
  const rules = resources.EmailAttachmentsBucket.Properties.LifecycleConfiguration.Rules;
  const tempRule = rules.find((r) => r.Prefix === 'temp/');
  assert.ok(tempRule, 'temp/ prefix expiration rule must still exist');
  assert.ok(tempRule.ExpirationInDays, 'temp/ prefix rule must declare ExpirationInDays');
  const noncurrentRule = rules.find((r) => r.NoncurrentVersionExpiration);
  assert.ok(noncurrentRule, 'a separate noncurrent-version expiration rule must also exist');
});

// ---------------------------------------------------------------------------
// CORS: exact per-role allowed methods, backed by code evidence (see rollout doc)
// ---------------------------------------------------------------------------

const expectedCors = {
  // PUT only: apps/web .../service-storage/util/upload.ts -> uploadToPresignedUrl() does a
  // `fetch(presignedUrl, { method: 'PUT' })` against the URL from
  // crates/documents/src/outbound/s3_upload_url.rs put_document_storage_presigned_url.
  DocumentsBucket: ['PUT'],
  // Mirrors the existing, already-deployed static-file-service bucket's own CORS rule verbatim:
  // infra/stacks/static-file-service/static-file-service.ts corsRules.allowedMethods.
  StaticBucket: ['GET', 'PUT', 'POST', 'DELETE', 'HEAD'],
  // PUT only: services/email_service/.../drafts/add_attachment.rs returns a presigned PUT
  // `upload_url`; apps/web/src/lib/queries/email/attachment.ts uploads via the same
  // uploadToPresignedUrl() fetch-PUT helper. Downloads use window.open (not CORS-gated), so no
  // GET/HEAD is added without its own fetch evidence.
  EmailAttachmentsBucket: ['PUT'],
  // PUT only: crates/documents/src/domain/service.rs calls put_docx_upload_presigned_url for
  // FileType::Docx, returned through the same upload.ts PUT flow as every other document type.
  DocxStagingBucket: ['PUT'],
  // PUT only: crates/projects/src/outbound/s3_upload_urls.rs
  // put_upload_zip_staging_presigned_url, returned by upload_folder.rs for direct client PUT.
  UploadStagingBucket: ['PUT'],
};

test('CORS is restricted to the single parameterized Station origin, never a wildcard', () => {
  for (const name of Object.keys(expectedCors)) {
    const cors = resources[name].Properties.CorsConfiguration;
    assert.ok(cors, `${name} is expected to expose a CORS configuration`);
    for (const rule of cors.CorsRules) {
      assert.deepEqual(rule.AllowedOrigins, [{ Ref: 'StationOriginUrl' }], `${name} CORS AllowedOrigins must be exactly the StationOriginUrl parameter, not a literal or wildcard`);
      assert.ok(!rule.AllowedOrigins.includes('*'), `${name} CORS must never allow '*'`);
    }
  }
});

test('each browser-facing bucket declares its own exact, evidenced set of allowed CORS methods (not a shared default)', () => {
  for (const [name, methods] of Object.entries(expectedCors)) {
    const cors = resources[name].Properties.CorsConfiguration;
    const actual = cors.CorsRules[0].AllowedMethods;
    assert.deepEqual([...actual].sort(), [...methods].sort(), `${name} AllowedMethods must exactly equal ${JSON.stringify(methods)} (evidence-backed), got ${JSON.stringify(actual)}`);
  }
});

test('server-to-server-only buckets never expose a CORS configuration', () => {
  const corsBuckets = new Set(Object.keys(expectedCors));
  const nonCorsBuckets = bucketResourceNames.filter((n) => !corsBuckets.has(n));
  assert.ok(nonCorsBuckets.length > 0, 'sanity check: there should be at least one non-CORS bucket (backup)');
  for (const name of nonCorsBuckets) {
    assert.equal(resources[name].Properties.CorsConfiguration, undefined, `${name} is server-to-server only and should not expose CORS`);
  }
});

test('StationOriginUrl parameter is constrained to validated https origins, with no hardcoded example host baked into the constraint text', () => {
  const param = tpl.Parameters.StationOriginUrl;
  assert.ok(param.AllowedPattern, 'StationOriginUrl must declare AllowedPattern');
  assert.match(param.AllowedPattern, /https/, 'StationOriginUrl pattern must require https');
  assert.doesNotMatch('http://insecure.example.com', new RegExp(param.AllowedPattern), 'plain http origins must be rejected by the pattern');
  assert.match('https://app.example.com', new RegExp(param.AllowedPattern), 'a valid https origin must be accepted by the pattern');
  const constraintText = `${param.ConstraintDescription ?? ''} ${param.Description ?? ''}`;
  assert.doesNotMatch(constraintText, /example\.com|station\.example/i, 'constraint/description text must not bake in a specific placeholder hostname');
});

// ---------------------------------------------------------------------------
// Backup bucket: generic destination, opt-in, writer/restore split, no false claims
// ---------------------------------------------------------------------------

test('backup bucket is disabled by default (no approved backup provider yet)', () => {
  assert.equal(tpl.Parameters.EnableBackupBucket.Default, 'false');
});

test('template does not claim the documents bucket is the only durable/canonical store', () => {
  const suspicious = /documents? is the only (durable|canonical)/i;
  assert.doesNotMatch(tpl.Description, suspicious);
});

test('a CloudFormation Rule requires a non-empty BackupBucketName whenever EnableBackupBucket is true', () => {
  assert.ok(tpl.Rules, 'template must declare a Rules section');
  const rule = tpl.Rules.BackupBucketNameRequiredWhenEnabled;
  assert.ok(rule, 'BackupBucketNameRequiredWhenEnabled rule must exist');
  assert.deepEqual(rule.RuleCondition, { 'Fn::Equals': [{ Ref: 'EnableBackupBucket' }, 'true'] });
  assert.ok(rule.Assertions.length >= 1, 'rule must declare at least one assertion');
  const assertion = JSON.stringify(rule.Assertions[0].Assert);
  assert.match(assertion, /BackupBucketName/, 'assertion must reference BackupBucketName');
  assert.match(assertion, /Fn::Not/, 'assertion must reject the empty-string case');
});

test('backup writer policy can write but can never delete backup objects', () => {
  const writer = resources.BackupWriterAccessPolicy;
  assert.ok(writer, 'BackupWriterAccessPolicy must exist');
  assert.equal(writer.Condition, 'CreateBackupBucket');
  const allActions = writer.Properties.PolicyDocument.Statement.flatMap((s) => (Array.isArray(s.Action) ? s.Action : [s.Action]));
  assert.ok(allActions.includes('s3:PutObject'), 'writer policy must allow s3:PutObject');
  for (const forbidden of ['s3:DeleteObject', 's3:DeleteObjectVersion', 's3:DeleteBucket', 's3:*']) {
    assert.ok(!allActions.includes(forbidden), `writer policy must not include ${forbidden}`);
  }
});

test('backup restore policy is read-only and separate from the writer policy', () => {
  const restore = resources.BackupRestoreAccessPolicy;
  assert.ok(restore, 'BackupRestoreAccessPolicy must exist');
  assert.equal(restore.Condition, 'CreateBackupBucket');
  const allActions = restore.Properties.PolicyDocument.Statement.flatMap((s) => (Array.isArray(s.Action) ? s.Action : [s.Action]));
  for (const action of allActions) {
    assert.ok(/^s3:(Get|List)/.test(action), `restore policy action ${action} must be read-only (Get*/List*)`);
  }
  assert.notEqual(resources.BackupWriterAccessPolicy, resources.BackupRestoreAccessPolicy);
});

test('this template does not wire any automatic copy/replication job into the backup bucket', () => {
  assert.equal(resources.BackupReplicationRule, undefined);
  const types = Object.values(resources).map((r) => r.Type);
  assert.ok(!types.includes('AWS::S3::Bucket::ReplicationConfiguration'));
  const serialized = JSON.stringify(resources);
  assert.ok(!serialized.includes('ReplicationConfiguration'), 'no S3 replication configuration should be wired to the backup bucket by this template');
});

// ---------------------------------------------------------------------------
// IAM: least privilege, nothing attached, backup isolated, multipart cleanup covered
// ---------------------------------------------------------------------------

test('backup access policies are never referenced by any app-facing access policy', () => {
  const appPolicies = [
    'DocumentsAppAccessPolicy',
    'StaticAppAccessPolicy',
    'EmailAttachmentsAppAccessPolicy',
    'DocxStagingAppAccessPolicy',
    'UploadStagingAppAccessPolicy',
    'NotificationQueueConsumerPolicy',
  ];
  for (const name of appPolicies) {
    const serialized = JSON.stringify(resources[name]);
    assert.ok(!serialized.includes('BackupBucketName'), `${name} must not reference the backup bucket`);
    assert.ok(!serialized.includes('BackupBucket"'), `${name} must not GetAtt/Ref the backup bucket resource`);
  }
});

test('no resource in this template attaches a policy to, or modifies, an existing IAM role', () => {
  const forbiddenTypes = new Set(['AWS::IAM::RolePolicyAttachment', 'AWS::IAM::RolePolicy', 'AWS::IAM::Role']);
  for (const [name, def] of Object.entries(resources)) {
    assert.ok(!forbiddenTypes.has(def.Type), `${name} has forbidden type ${def.Type}; this template must only emit standalone, unattached managed policies`);
  }
});

test('all IAM managed policies avoid broad wildcard admin access (no Action:"*" combined with Resource:"*")', () => {
  for (const [name, def] of Object.entries(resources)) {
    if (def.Type !== 'AWS::IAM::ManagedPolicy') continue;
    for (const stmt of def.Properties.PolicyDocument.Statement) {
      const actions = Array.isArray(stmt.Action) ? stmt.Action : [stmt.Action];
      const resourcesList = Array.isArray(stmt.Resource) ? stmt.Resource : [stmt.Resource];
      const hasWildcardAction = actions.some((a) => a === '*');
      const hasWildcardResource = resourcesList.some((r) => r === '*');
      assert.ok(!(hasWildcardAction && hasWildcardResource), `${name} statement grants wildcard action AND wildcard resource`);
      assert.ok(!hasWildcardResource, `${name} statement must scope Resource, not use '*'`);
    }
  }
});

test('app access policies (documents/static/email/docx-staging/upload-staging) grant multipart cleanup actions', () => {
  const appPolicies = [
    'DocumentsAppAccessPolicy',
    'StaticAppAccessPolicy',
    'EmailAttachmentsAppAccessPolicy',
    'DocxStagingAppAccessPolicy',
    'UploadStagingAppAccessPolicy',
  ];
  for (const name of appPolicies) {
    const stmts = resources[name].Properties.PolicyDocument.Statement;
    const allActions = stmts.flatMap((s) => (Array.isArray(s.Action) ? s.Action : [s.Action]));
    assert.ok(allActions.includes('s3:AbortMultipartUpload'), `${name} must allow s3:AbortMultipartUpload on its object resource`);
    assert.ok(allActions.includes('s3:ListMultipartUploadParts'), `${name} must allow s3:ListMultipartUploadParts on its object resource`);
    assert.ok(allActions.includes('s3:ListBucketMultipartUploads'), `${name} must allow s3:ListBucketMultipartUploads on its bucket resource`);
    for (const stmt of stmts) {
      const stmtActions = Array.isArray(stmt.Action) ? stmt.Action : [stmt.Action];
      if (stmtActions.some((a) => a.includes('MultipartUpload'))) {
        const resourcesList = Array.isArray(stmt.Resource) ? stmt.Resource : [stmt.Resource];
        assert.ok(!resourcesList.includes('*'), `${name} multipart-cleanup statement must not use a wildcard Resource`);
      }
    }
  }
});

// ---------------------------------------------------------------------------
// Queue notification policy scoping
// ---------------------------------------------------------------------------

test('S3-to-SQS notification queue policies are scoped to a specific source bucket ARN and source account', () => {
  for (const policyName of ['DocumentsQueuePolicy', 'StaticQueuePolicy']) {
    const stmt = resources[policyName].Properties.PolicyDocument.Statement[0];
    assert.equal(stmt.Principal.Service, 's3.amazonaws.com');
    assert.ok(stmt.Condition.ArnEquals['aws:SourceArn'], `${policyName} must scope by aws:SourceArn`);
    assert.ok(stmt.Condition.StringEquals['aws:SourceAccount'], `${policyName} must scope by aws:SourceAccount`);
  }
});

test('documents and static queues declare a redrive policy pointing at a dead-letter queue', () => {
  for (const [queueName, dlqName] of [['DocumentsQueue', 'DocumentsDlq'], ['StaticQueue', 'StaticDlq']]) {
    const redrive = resources[queueName].Properties.RedrivePolicy;
    assert.ok(redrive, `${queueName} must declare a RedrivePolicy`);
    assert.deepEqual(redrive.deadLetterTargetArn, { 'Fn::GetAtt': [dlqName, 'Arn'] });
    assert.ok(redrive.maxReceiveCount, `${queueName} RedrivePolicy must set maxReceiveCount`);
  }
});

test('all queues use SQS-managed SSE encryption', () => {
  for (const [name, def] of Object.entries(resources)) {
    if (def.Type !== 'AWS::SQS::Queue') continue;
    assert.equal(def.Properties.SqsManagedSseEnabled, true, `${name} must enable SqsManagedSseEnabled`);
  }
});

test('SQS numeric parameters are bounded within real AWS service limits', () => {
  const p = tpl.Parameters;
  assert.equal(p.QueueVisibilityTimeoutSeconds.MinValue, 0);
  assert.equal(p.QueueVisibilityTimeoutSeconds.MaxValue, 43200);
  assert.equal(p.QueueMessageRetentionSeconds.MinValue, 60);
  assert.equal(p.QueueMessageRetentionSeconds.MaxValue, 1209600);
  assert.equal(p.DlqMessageRetentionSeconds.MinValue, 60);
  assert.equal(p.DlqMessageRetentionSeconds.MaxValue, 1209600);
  assert.equal(p.MaxReceiveCount.MinValue, 1);
  assert.equal(p.MaxReceiveCount.MaxValue, 1000);
});

test('bucket/queue/table name parameters declare a basic naming-rule regex', () => {
  const p = tpl.Parameters;
  const s3Named = ['DocumentsBucketName', 'StaticBucketName', 'EmailAttachmentsBucketName', 'DocxStagingBucketName', 'UploadStagingBucketName'];
  for (const name of s3Named) {
    assert.ok(p[name].AllowedPattern, `${name} must declare AllowedPattern`);
    assert.match('my-valid-bucket-name', new RegExp(p[name].AllowedPattern));
    assert.doesNotMatch('Invalid_Bucket_Name!', new RegExp(p[name].AllowedPattern));
  }
  assert.ok(p.BackupBucketName.AllowedPattern, 'BackupBucketName must declare AllowedPattern');
  assert.match('', new RegExp(p.BackupBucketName.AllowedPattern), 'BackupBucketName pattern must still allow empty string (disabled case)');

  const sqsNamed = ['DocumentsQueueName', 'DocumentsDlqName', 'StaticQueueName', 'StaticDlqName'];
  for (const name of sqsNamed) {
    assert.ok(p[name].AllowedPattern, `${name} must declare AllowedPattern`);
    assert.match('valid-queue-name_1', new RegExp(p[name].AllowedPattern));
    assert.doesNotMatch('invalid queue name!', new RegExp(p[name].AllowedPattern));
  }

  assert.ok(p.StaticMetadataTableName.AllowedPattern, 'StaticMetadataTableName must declare AllowedPattern');
  assert.match('valid_table-name.1', new RegExp(p.StaticMetadataTableName.AllowedPattern));
});

// ---------------------------------------------------------------------------
// Static metadata table schema, matching services/static_file_service model.rs
// ---------------------------------------------------------------------------

test('static metadata table key schema matches the Rust MetadataObject model (file_id: String partition key)', () => {
  const table = resources.StaticMetadataTable.Properties;
  assert.deepEqual(table.KeySchema, [{ AttributeName: 'file_id', KeyType: 'HASH' }]);
  const fileIdAttr = table.AttributeDefinitions.find((a) => a.AttributeName === 'file_id');
  assert.ok(fileIdAttr, 'file_id attribute definition missing');
  assert.equal(fileIdAttr.AttributeType, 'S', 'file_id must be a String (S) type');
  assert.equal(table.KeySchema.length, 1, 'file_id must be the sole primary key (no sort key), matching the existing table');
});

test('static metadata table is on-demand billed with point-in-time recovery and SSE enabled', () => {
  const table = resources.StaticMetadataTable.Properties;
  assert.equal(table.BillingMode, 'PAY_PER_REQUEST');
  assert.equal(table.PointInTimeRecoverySpecification.PointInTimeRecoveryEnabled, true);
  assert.equal(table.SSESpecification.SSEEnabled, true);
  assert.equal(resources.StaticMetadataTable.DeletionPolicy, 'Retain');
});

test('static metadata table preserves the existing owner-index GSI', () => {
  const gsis = resources.StaticMetadataTable.Properties.GlobalSecondaryIndexes;
  const ownerIndex = gsis.find((g) => g.IndexName === 'owner-index');
  assert.ok(ownerIndex, 'owner-index GSI must be preserved');
  assert.deepEqual(ownerIndex.KeySchema, [{ AttributeName: 'owner_id', KeyType: 'HASH' }]);
});

// ---------------------------------------------------------------------------
// Object key preservation (no tenant-prefix rewriting logic anywhere in the template)
// ---------------------------------------------------------------------------

test('template does not introduce generated tenant-prefix key rewriting', () => {
  const suspicious = /tenant[-_]?prefix|rewrite[-_]?key|key[-_]?rewrite/i;
  assert.doesNotMatch(raw, suspicious, 'Found suspicious tenant-prefix/key-rewrite logic; object keys must be preserved as-is');
});

test('all five bucket names and the backup bucket name are plain parameters, not derived/generated', () => {
  for (const p of ['DocumentsBucketName', 'StaticBucketName', 'EmailAttachmentsBucketName', 'DocxStagingBucketName', 'UploadStagingBucketName', 'BackupBucketName']) {
    assert.equal(tpl.Parameters[p].Type, 'String');
  }
});
