# Gmail Refresh Handler

To keep inbox update push notifications coming from email providers, we need to hit provider-specific endpoints like
[users.watch](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users/watch) for each 
subscribed user once per day. The refresh handler is triggered by eventbridge once per hour, grabbing a subset 
of the emails that we have a subscription active for. It puts these emails on an SQS queue, which then get picked 
up by a listener in email-service that hits the necessary endpoint for each email. Each email is processed once per day.

## Host Microsoft run-once mode

The normal no-argument entrypoint remains the AWS Lambda runtime, including its existing Gmail refresh, health-check, Microsoft discovery, and timed-deletion behavior. A host scheduler can explicitly schedule only Microsoft mailbox discovery once with:

```sh
email_refresh_handler --microsoft-once
```

This mode uses the same configuration, database connection, queue client, logging, Microsoft bucketing, and discovery message as Lambda mode. It does not run Gmail refresh, Gmail health checks, or timed deletion, and it does not loop. A database or queue error is returned so the process exits nonzero. The external scheduler must prevent overlapping invocations.
