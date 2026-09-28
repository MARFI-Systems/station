use std::num::{NonZeroU16, NonZeroUsize};
use std::time::Duration;

use url::Url;
use wiremock::matchers::{header, headers, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn client_config(server: &MockServer) -> MicrosoftGraphMailClientConfig {
    MicrosoftGraphMailClientConfig::for_origin(
        Url::parse(&format!("{}/", server.uri())).expect("wiremock URI is valid"),
    )
    .with_page_size(NonZeroU16::new(2).expect("non-zero"))
}

fn client(server: &MockServer) -> MicrosoftGraphMailClient {
    MicrosoftGraphMailClient::with_config(client_config(server)).expect("test config is valid")
}

fn token() -> AccessToken {
    AccessToken::new("synthetic-token")
}

fn message_json(id: &str, parent_folder_id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "changeKey": "change-1",
        "conversationId": "conversation-1",
        "parentFolderId": parent_folder_id,
        "internetMessageId": "<fixture@example.test>",
        "subject": "Synthetic fixture",
        "bodyPreview": "Fixture preview",
        "from": {"emailAddress": {"name": "Sender", "address": "sender@example.test"}},
        "sender": {"emailAddress": {"name": "Sender", "address": "sender@example.test"}},
        "toRecipients": [
            {"emailAddress": {"name": "Recipient", "address": "recipient@example.test"}}
        ],
        "ccRecipients": [],
        "bccRecipients": [],
        "receivedDateTime": "2026-09-27T12:00:00Z",
        "sentDateTime": "2026-09-27T11:59:00Z",
        "isRead": false,
        "isDraft": false,
        "hasAttachments": true,
        "importance": "normal",
        "categories": ["Synthetic"]
    })
}

#[tokio::test]
async fn lists_folders_in_bounded_resumable_pages() {
    let server = MockServer::start().await;
    let first_path = "/v1.0/me/mailFolders";
    let second_link = format!("{}{first_path}?$skiptoken=next", server.uri());

    Mock::given(method("GET"))
        .and(path(first_path))
        .and(query_param("includeHiddenFolders", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{
                "id": "inbox",
                "displayName": "Inbox",
                "parentFolderId": "root",
                "childFolderCount": 1,
                "unreadItemCount": 2,
                "totalItemCount": 3,
                "isHidden": false
            }],
            "@odata.nextLink": second_link
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(first_path))
        .and(query_param("$skiptoken", "next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{
                "id": "archive",
                "displayName": "Archive",
                "parentFolderId": "root",
                "childFolderCount": 0,
                "unreadItemCount": 0,
                "totalItemCount": 4,
                "isHidden": true
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = MicrosoftGraphMailClient::with_config(
        client_config(&server).with_max_pages_per_call(NonZeroU16::new(1).expect("non-zero")),
    )
    .unwrap();
    let first = client
        .list_mail_folders(&token(), None, None)
        .await
        .unwrap();
    assert_eq!(first.pages_fetched, 1);
    assert_eq!(first.folders[0].id, "inbox");
    let second = client
        .list_mail_folders(&token(), None, first.next_page.as_ref())
        .await
        .unwrap();
    assert_eq!(second.pages_fetched, 1);
    assert_eq!(second.folders[0].parent_folder_id.as_deref(), Some("root"));
    assert!(second.folders[0].is_hidden);
    assert!(second.next_page.is_none());
}

#[tokio::test]
async fn recursively_walks_delegated_me_folders_with_resumable_state() {
    let server = MockServer::start().await;
    let root_path = "/v1.0/me/mailFolders";
    let next_link = format!("{}{root_path}?$skiptoken=root-next", server.uri());

    Mock::given(method("GET"))
        .and(path(root_path))
        .and(query_param("includeHiddenFolders", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{
                "id": "projects",
                "displayName": "Projects",
                "parentFolderId": "root",
                "childFolderCount": 1,
                "unreadItemCount": 0,
                "totalItemCount": 1,
                "isHidden": false
            }],
            "@odata.nextLink": next_link
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(root_path))
        .and(query_param("$skiptoken", "root-next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{
                "id": "archive",
                "displayName": "Archive",
                "parentFolderId": "root",
                "childFolderCount": 0,
                "unreadItemCount": 0,
                "totalItemCount": 2,
                "isHidden": false
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/mailFolders/projects/childFolders"))
        .and(query_param("includeHiddenFolders", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{
                "id": "projects-2026",
                "displayName": "2026/Plans",
                "parentFolderId": "projects",
                "childFolderCount": 0,
                "unreadItemCount": 1,
                "totalItemCount": 1,
                "isHidden": false
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = MicrosoftGraphMailClient::with_config(
        client_config(&server).with_max_pages_per_call(NonZeroU16::new(1).expect("non-zero")),
    )
    .unwrap();
    let first = client.walk_mail_folders(&token(), None).await.unwrap();
    assert_eq!(first.folders[0].path, vec!["Projects".to_string()]);
    let first_cursor = first.next_cursor.unwrap();
    assert_eq!(first_cursor.pending_parent_count(), 1);

    let second = client
        .walk_mail_folders(&token(), Some(&first_cursor))
        .await
        .unwrap();
    assert_eq!(second.folders[0].path, vec!["Archive".to_string()]);
    let second_cursor = second.next_cursor.unwrap();
    assert_eq!(second_cursor.pending_parent_count(), 0);

    let third = client
        .walk_mail_folders(&token(), Some(&second_cursor))
        .await
        .unwrap();
    assert_eq!(
        third.folders[0].path,
        vec!["Projects".to_string(), "2026/Plans".to_string()]
    );
    assert!(third.next_cursor.is_none());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request.url.path().starts_with("/v1.0/me/"))
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.url.path().contains("/users/"))
    );
}

#[tokio::test]
async fn bounds_recursive_folder_walk_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/mailFolders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [
                {
                    "id": "one",
                    "displayName": "One",
                    "parentFolderId": "root",
                    "childFolderCount": 0,
                    "unreadItemCount": 0,
                    "totalItemCount": 0,
                    "isHidden": false
                },
                {
                    "id": "two",
                    "displayName": "Two",
                    "parentFolderId": "root",
                    "childFolderCount": 0,
                    "unreadItemCount": 0,
                    "totalItemCount": 0,
                    "isHidden": false
                }
            ]
        })))
        .mount(&server)
        .await;
    let client = MicrosoftGraphMailClient::with_config(
        client_config(&server)
            .with_max_folder_walk_folders(NonZeroUsize::new(1).expect("non-zero")),
    )
    .unwrap();
    assert!(matches!(
        client.walk_mail_folders(&token(), None).await,
        Err(EmailApiError::Permanent { .. })
    ));
}

#[tokio::test]
async fn paginates_folder_delta_and_preserves_move_delete_tombstones() {
    let server = MockServer::start().await;
    let delta_path = "/v1.0/me/mailFolders/inbox/messages/delta";
    let next_link = format!("{}{delta_path}?$skiptoken=next", server.uri());
    let delta_link = format!("{}{delta_path}?$deltatoken=done", server.uri());

    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$select", DELTA_MESSAGE_SELECT))
        .and(headers(
            "prefer",
            vec!["odata.maxpagesize=2", "IdType=\"ImmutableId\""],
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [message_json("message-1", "inbox")],
            "@odata.nextLink": next_link
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$skiptoken", "next"))
        .and(headers(
            "prefer",
            vec!["odata.maxpagesize=2", "IdType=\"ImmutableId\""],
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [{"id": "message-moved-or-deleted", "@removed": {"reason": "deleted"}}],
            "@odata.deltaLink": delta_link
        })))
        .expect(1)
        .mount(&server)
        .await;

    let batch = client(&server)
        .list_folder_message_delta(&token(), "inbox", None)
        .await
        .unwrap();
    assert_eq!(batch.pages_fetched, 2);
    assert_eq!(batch.cursor.kind(), MicrosoftGraphDeltaCursorKind::Delta);
    assert!(matches!(
        &batch.changes[0],
        MicrosoftGraphMessageDeltaChange::Upsert(message)
            if message.parent_folder_id.as_deref() == Some("inbox")
    ));
    assert!(matches!(
        &batch.changes[1],
        MicrosoftGraphMessageDeltaChange::Removed(removed)
            if removed.id == "message-moved-or-deleted" && removed.reason == "deleted"
    ));
}

#[tokio::test]
async fn restarts_once_when_a_per_folder_delta_cursor_expires() {
    let server = MockServer::start().await;
    let delta_path = "/v1.0/me/mailFolders/inbox/messages/delta";
    let expired_cursor = MicrosoftGraphDeltaCursor::new(
        MicrosoftGraphDeltaCursorKind::Delta,
        format!("{}{delta_path}?$deltatoken=expired", server.uri()),
    );
    let replacement_delta = format!("{}{delta_path}?$deltatoken=replacement", server.uri());

    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$deltatoken", "expired"))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$select", DELTA_MESSAGE_SELECT))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [message_json("current-message", "inbox")],
            "@odata.deltaLink": replacement_delta
        })))
        .expect(1)
        .mount(&server)
        .await;

    let recovery = client(&server)
        .resume_folder_message_delta(&token(), "inbox", &expired_cursor)
        .await
        .unwrap();
    assert!(recovery.requires_folder_rebuild());
    assert!(matches!(
        &recovery.batch().changes[0],
        MicrosoftGraphMessageDeltaChange::Upsert(message) if message.id == "current-message"
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    let failed_restart_server = MockServer::start().await;
    let failed_cursor = MicrosoftGraphDeltaCursor::new(
        MicrosoftGraphDeltaCursorKind::Delta,
        format!(
            "{}{delta_path}?$deltatoken=expired",
            failed_restart_server.uri()
        ),
    );
    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$deltatoken", "expired"))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount(&failed_restart_server)
        .await;
    Mock::given(method("GET"))
        .and(path(delta_path))
        .and(query_param("$select", DELTA_MESSAGE_SELECT))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount(&failed_restart_server)
        .await;
    assert_eq!(
        client(&failed_restart_server)
            .resume_folder_message_delta(&token(), "inbox", &failed_cursor)
            .await
            .unwrap_err(),
        EmailApiError::OutdatedCursor
    );
    assert_eq!(
        failed_restart_server
            .received_requests()
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn rejects_cross_origin_and_cross_mailbox_continuations_before_request() {
    let server = MockServer::start().await;
    let client = client(&server);
    let cross_origin = MicrosoftGraphDeltaCursor::new(
        MicrosoftGraphDeltaCursorKind::Continuation,
        "https://example.invalid/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=x".to_string(),
    );
    assert!(matches!(
        client
            .list_folder_message_delta(&token(), "inbox", Some(&cross_origin))
            .await,
        Err(EmailApiError::Permanent { .. })
    ));

    let cross_mailbox = MicrosoftGraphDeltaCursor::new(
        MicrosoftGraphDeltaCursorKind::Continuation,
        format!(
            "{}/v1.0/users/other/mailFolders/inbox/messages/delta?$skiptoken=x",
            server.uri()
        ),
    );
    assert!(matches!(
        client
            .list_folder_message_delta(&token(), "inbox", Some(&cross_mailbox))
            .await,
        Err(EmailApiError::Permanent { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_untrusted_next_link_before_attaching_bearer_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/mailFolders/inbox/messages/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [],
            "@odata.nextLink": "https://example.invalid/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=x"
        })))
        .expect(1)
        .mount(&server)
        .await;

    assert!(matches!(
        client(&server)
            .list_folder_message_delta(&token(), "inbox", None)
            .await,
        Err(EmailApiError::Permanent { .. })
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn maps_retry_after_and_expired_delta_cursor() {
    let server = MockServer::start().await;
    let delta_path = "/v1.0/me/mailFolders/inbox/messages/delta";
    Mock::given(method("GET"))
        .and(path(delta_path))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
        .expect(1)
        .mount(&server)
        .await;

    assert_eq!(
        client(&server)
            .list_folder_message_delta(&token(), "inbox", None)
            .await
            .unwrap_err(),
        EmailApiError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
            origin: RateLimitOrigin::Provider,
        }
    );

    let expired_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(delta_path))
        .respond_with(ResponseTemplate::new(410))
        .expect(1)
        .mount(&expired_server)
        .await;
    assert_eq!(
        client(&expired_server)
            .list_folder_message_delta(&token(), "inbox", None)
            .await
            .unwrap_err(),
        EmailApiError::OutdatedCursor
    );
}

#[tokio::test]
async fn gets_typed_message_and_bounds_attachment_bytes() {
    let server = MockServer::start().await;
    let mut message = message_json("message-1", "inbox");
    message["body"] = serde_json::json!({"contentType": "html", "content": "<b>untrusted</b>"});
    message["attachments"] = serde_json::json!([{
        "@odata.type": "#microsoft.graph.fileAttachment",
        "id": "attachment-1",
        "name": "fixture.txt",
        "contentType": "text/plain",
        "size": 3,
        "isInline": false,
        "contentId": null
    }]);
    Mock::given(method("GET"))
        .and(path("/v1.0/me/messages/message-1"))
        .and(header("prefer", "IdType=\"ImmutableId\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(message))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/v1.0/me/messages/message-1/attachments/attachment-1/$value",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(b"abc".to_vec(), "application/octet-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let message = client
        .get_message(&token(), "message-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message.attachments[0].kind,
        MicrosoftGraphAttachmentKind::File
    );
    assert_eq!(
        message.body.unwrap().content_type,
        MicrosoftGraphBodyContentType::Html
    );
    assert_eq!(
        client
            .get_attachment_bytes(&token(), "message-1", "attachment-1")
            .await
            .unwrap(),
        b"abc"
    );

    let oversized_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/v1.0/me/messages/message-1/attachments/attachment-1/$value",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"too-large".to_vec(), "application/octet-stream"),
        )
        .mount(&oversized_server)
        .await;
    let bounded = MicrosoftGraphMailClient::with_config(
        client_config(&oversized_server)
            .with_max_attachment_bytes(NonZeroUsize::new(4).expect("non-zero")),
    )
    .unwrap();
    assert!(matches!(
        bounded
            .get_attachment_bytes(&token(), "message-1", "attachment-1")
            .await,
        Err(EmailApiError::Permanent { .. })
    ));
}

#[tokio::test]
async fn returns_none_for_message_deletion_race() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/messages/deleted"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(
        client(&server)
            .get_message(&token(), "deleted")
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn read_only_public_contract_and_path_encoding_compile_sanity() {
    fn requires_read_only_attachment_port<T: MailboxAttachmentClient>(_: &T) {}

    let client = MicrosoftGraphMailClient::new().unwrap();
    requires_read_only_attachment_port(&client);
    let _ = MicrosoftGraphMailClient::list_mail_folders;
    let _ = MicrosoftGraphMailClient::walk_mail_folders;
    let _ = MicrosoftGraphMailClient::list_folder_message_delta;
    let _ = MicrosoftGraphMailClient::resume_folder_message_delta;
    let _ = MicrosoftGraphMailClient::get_message;
    let _ = MicrosoftGraphMailClient::get_attachment_bytes;

    let encoded = client.graph_url(&["me", "messages", "id/with?reserved#characters"]);
    assert_eq!(
        encoded.path(),
        "/v1.0/me/messages/id%2Fwith%3Freserved%23characters"
    );
    assert_eq!(format!("{:?}", token()), "AccessToken([REDACTED])");
}

#[tokio::test]
async fn rejects_redirects_without_forwarding_mailbox_bearer() {
    let server = MockServer::start().await;
    let redirected_path = "/v1.0/users/another-mailbox/messages";
    Mock::given(method("GET"))
        .and(path("/v1.0/me/messages/message-1"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}{redirected_path}", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(redirected_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_json("message-1", "inbox")))
        .expect(0)
        .mount(&server)
        .await;

    let result = client(&server).get_message(&token(), "message-1").await;
    assert!(matches!(result, Err(EmailApiError::Permanent { .. })));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
