use crate::{
    AuthServiceClient,
    error::{AuthServiceClientError, GenericErrorResponse},
};

/// Verified Microsoft mailbox identity, stable by tenant plus Entra object id.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxIdentity {
    /// Microsoft-verified normalized mailbox address.
    pub email: String,
    /// Verified Entra tenant identifier.
    pub tenant_id: String,
    /// Verified stable Entra object identifier.
    pub object_id: String,
}

/// A short-lived delegated Microsoft Graph token. Deliberately has no `Debug` implementation.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxAccessToken {
    /// Delegated Graph bearer token.
    pub access_token: String,
    /// Provider token type, normally `Bearer`.
    pub token_type: String,
    /// Remaining lifetime in seconds.
    pub expires_in: u64,
    /// Verified mailbox identity associated with this token.
    pub mailbox: MicrosoftMailboxIdentity,
}

/// Safe owner-scoped mailbox status with no tokens.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftMailboxStatus {
    /// Whether an active purpose-versioned mailbox grant exists.
    pub connected: bool,
    /// Verified identity when connected.
    pub mailbox: Option<MicrosoftMailboxIdentity>,
}

impl AuthServiceClient {
    /// Fetches a read-only Microsoft mailbox token for the exact Station owner.
    pub async fn get_microsoft_mailbox_access_token(
        &self,
        fusionauth_user_id: &str,
    ) -> Result<MicrosoftMailboxAccessToken, AuthServiceClientError> {
        let response = self
            .client
            .get(format!(
                "{}/internal/microsoft_mailbox_access_token",
                self.url
            ))
            .query(&[("fusionauth_user_id", fusionauth_user_id)])
            .send()
            .await
            .map_err(|error| AuthServiceClientError::RequestBuildError {
                details: error.to_string(),
            })?;
        parse_json(response).await
    }

    /// Returns safe Microsoft mailbox connection metadata for the exact Station owner.
    pub async fn get_microsoft_mailbox_status(
        &self,
        fusionauth_user_id: &str,
    ) -> Result<MicrosoftMailboxStatus, AuthServiceClientError> {
        let response = self
            .client
            .get(format!("{}/internal/microsoft_mailbox_status", self.url))
            .query(&[("fusionauth_user_id", fusionauth_user_id)])
            .send()
            .await
            .map_err(|error| AuthServiceClientError::RequestBuildError {
                details: error.to_string(),
            })?;
        parse_json(response).await
    }

    /// Deletes the owner's local Microsoft grant only; it performs no remote Microsoft writes.
    pub async fn disconnect_microsoft_mailbox(
        &self,
        fusionauth_user_id: &str,
    ) -> Result<(), AuthServiceClientError> {
        let response = self
            .client
            .delete(format!("{}/internal/microsoft_mailbox_grant", self.url))
            .query(&[("fusionauth_user_id", fusionauth_user_id)])
            .send()
            .await
            .map_err(|error| AuthServiceClientError::RequestBuildError {
                details: error.to_string(),
            })?;
        match response.status() {
            reqwest::StatusCode::NO_CONTENT => Ok(()),
            reqwest::StatusCode::UNAUTHORIZED => Err(AuthServiceClientError::Unauthorized),
            reqwest::StatusCode::FORBIDDEN => Err(AuthServiceClientError::Forbidden),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(AuthServiceClientError::Generic(GenericErrorResponse {
                message: response.text().await.unwrap_or_default(),
            })),
        }
    }
}

async fn parse_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, AuthServiceClientError> {
    match response.status() {
        reqwest::StatusCode::OK => response.json().await.map_err(|error| {
            AuthServiceClientError::Generic(GenericErrorResponse {
                message: error.to_string(),
            })
        }),
        reqwest::StatusCode::UNAUTHORIZED => Err(AuthServiceClientError::Unauthorized),
        reqwest::StatusCode::FORBIDDEN => Err(AuthServiceClientError::Forbidden),
        reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR | reqwest::StatusCode::BAD_GATEWAY => {
            Err(AuthServiceClientError::InternalServerError {
                details: response.text().await.unwrap_or_default(),
            })
        }
        _ => Err(AuthServiceClientError::Generic(GenericErrorResponse {
            message: response.text().await.unwrap_or_default(),
        })),
    }
}
