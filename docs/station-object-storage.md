# Station object storage configuration

Station keeps the shared AWS configuration unchanged for queues, secrets, DynamoDB, and local development. S3 can be separated only through an explicit object-storage mode.

## Modes

- Unset or `legacy`: preserves the existing behavior. `LOCAL_AWS_URL` controls S3 together with the other AWS clients, including LocalStack test credentials and URL transforms.
- `aws`: uses AWS S3 independently. `OBJECT_STORAGE_REGION` is required. Dedicated object-storage credentials are optional; when omitted, the normal AWS credential chain supports a scoped workload role or configured AWS profile. If `LOCAL_AWS_URL` is also set, ambient `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, or `AWS_SESSION_TOKEN` values are rejected so development credentials cannot sign real S3 requests; remove those ambient fields and use an IAM role, or provide dedicated object-storage credentials.
- `s3-compatible`: uses a provider-specific S3 endpoint. Region, HTTPS endpoint, and dedicated object-storage credentials are required. Path-style requests are enabled.

Provider variables:

- `OBJECT_STORAGE_MODE`
- `OBJECT_STORAGE_REGION`
- `OBJECT_STORAGE_ENDPOINT_URL`
- `OBJECT_STORAGE_ACCESS_KEY_ID`
- `OBJECT_STORAGE_SECRET_ACCESS_KEY`
- `OBJECT_STORAGE_SESSION_TOKEN` (optional, only with the access-key pair)

Provider variables without an explicit remote mode are rejected. Partial credentials, non-TLS compatible endpoints, endpoint user information, endpoint paths, and endpoint query strings are rejected. Configuration errors name variables but do not print credential values.

When `aws` or `s3-compatible` is selected, S3 presigned URLs are not rewritten even if `LOCAL_AWS_URL` remains enabled for other AWS services. Inherited generic and S3-specific AWS endpoint overrides are cleared before applying the selected remote endpoint policy. The generic `is_local_aws()` behavior is unchanged; S3-aware callers can use `is_local_s3()`, and consumers can use `is_custom_s3_configured()` to detect a valid explicit remote mode. Existing callers that use the generic local flag for S3-adjacent behavior still require a caller audit before rollout.

## Limits and rollout gates

This change configures the S3 client only. It does not migrate objects, buckets, metadata, event delivery, finalizers, or collaborative document state. It does not change checksum behavior.

An S3-compatible provider, including Backblaze B2, is not considered compatible until live tests pass for the exact presigned upload and download flow, encoded object keys, checksum headers and trailers, multipart behavior where used, and the production event/finalizer path. Do not claim provider compatibility from configuration or unit tests alone.

Use least-privilege credentials or workload roles scoped to the required buckets and actions. Keep credentials outside source control and documentation.
