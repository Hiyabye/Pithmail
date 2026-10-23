//! The sole application HTTP boundary: fixed Google endpoints, no redirects/proxies.
use reqwest::{redirect::Policy, Client, Response, Url};
use std::time::Duration;

#[derive(Clone)]
pub struct Network {
    client: Client,
    #[cfg(test)]
    token_endpoint: Option<Url>,
    #[cfg(test)]
    gmail_endpoint: Option<Url>,
}

impl Network {
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| "Could not initialize secure networking.".to_string())?;
        Ok(Self {
            client,
            #[cfg(test)]
            token_endpoint: None,
            #[cfg(test)]
            gmail_endpoint: None,
        })
    }

    pub async fn gmail(
        &self,
        path: &str,
        query: &[(&str, String)],
        bearer: &str,
    ) -> Result<Response, String> {
        let url = gmail_url(path, query)?;
        #[cfg(test)]
        let url = if let Some(endpoint) = &self.gmail_endpoint {
            let mut local = endpoint.clone();
            local.set_path(url.path());
            local.set_query(url.query());
            local
        } else {
            url
        };
        self.client
            .get(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|_| "Gmail could not be reached. Check your connection and retry.".into())
    }

    pub async fn token(&self, form: &[(String, String)]) -> Result<Response, String> {
        let endpoint = "https://oauth2.googleapis.com/token";
        #[cfg(test)]
        let endpoint = self
            .token_endpoint
            .as_ref()
            .map(Url::as_str)
            .unwrap_or(endpoint);
        self.client
            .post(endpoint)
            .form(form)
            .send()
            .await
            .map_err(|_| {
                "Google authentication could not be reached. Check your connection and retry."
                    .into()
            })
    }

    #[cfg(test)]
    pub(crate) fn test_gmail_endpoint(endpoint: Url) -> Self {
        assert_eq!(endpoint.scheme(), "http");
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        let mut network = Self::test_token_endpoint(endpoint.join("/token").unwrap());
        network.gmail_endpoint = Some(endpoint);
        network
    }

    #[cfg(test)]
    pub(crate) fn test_token_endpoint(endpoint: Url) -> Self {
        assert_eq!(endpoint.scheme(), "http");
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        Self {
            client: Client::builder()
                .redirect(Policy::none())
                .no_proxy()
                .build()
                .unwrap(),
            token_endpoint: Some(endpoint),
            gmail_endpoint: None,
        }
    }
}

fn gmail_url(path: &str, query: &[(&str, String)]) -> Result<Url, String> {
    // Callers cannot replace the origin or introduce query, user-info or dot segments.
    let suffix = path
        .strip_prefix("/gmail/v1/users/me")
        .ok_or("Network policy refused this endpoint.")?;
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return Err("Network policy refused this endpoint.".into());
    }
    if path.split('/').any(|part| part == "." || part == "..")
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-".contains(&byte))
    {
        return Err("Network policy refused this endpoint.".into());
    }
    let mut url = Url::parse(&format!("https://gmail.googleapis.com{path}"))
        .map_err(|_| "Invalid Gmail endpoint.")?;
    url.query_pairs_mut()
        .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
    Ok(url)
}

pub async fn read_limited(mut response: Response, max: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err("Google response exceeds the supported size limit.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Google response was interrupted. Retry the operation.".to_string())?
    {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err("Google response exceeds the supported size limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_policy_rejects_origin_and_path_escape() {
        for path in [
            "https://evil.test/gmail/v1/users/me",
            "//evil.test",
            "/gmail/v1/users/meevil",
            "/gmail/v1/users/me/../other",
            "/gmail/v1/users/me/messages?x=1",
            "/gmail/v1/users/me/%2e%2e",
            "/gmail/v1/users/me/messages#x",
            "/gmail/v1/users/me/messages\\other",
        ] {
            assert!(gmail_url(path, &[]).is_err(), "{path}");
        }
        let url = gmail_url(
            "/gmail/v1/users/me/messages/ab12",
            &[("pageToken", "x&other=1".into())],
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("gmail.googleapis.com"));
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("pageToken".into(), "x&other=1".into())]
        );
    }
    #[tokio::test]
    async fn response_limits_and_redirect_policy_are_enforced() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        for (response, limit, expected_error) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",4,true),
            ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\n123\r\n3\r\n456\r\n0\r\n\r\n",5,true),
            ("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",5,false),
        ] {
            let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
            let address=listener.local_addr().unwrap();
            let server=tokio::spawn(async move {let(mut stream,_)=listener.accept().await.unwrap();let mut request=[0u8;1024];let _=stream.read(&mut request).await;stream.write_all(response.as_bytes()).await.unwrap();});
            let client=Network::test_token_endpoint(Url::parse(&format!("http://{address}/token")).unwrap());
            let response=client.token(&[]).await.unwrap();
            assert_eq!(read_limited(response,limit).await.is_err(),expected_error);
            server.await.unwrap();
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            stream.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client =
            Network::test_token_endpoint(Url::parse(&format!("http://{address}/token")).unwrap());
        assert_eq!(client.token(&[]).await.unwrap().status().as_u16(), 307);
        server.await.unwrap();
    }
    #[test]
    fn client_can_be_initialized_without_contacting_google() {
        Network::new().unwrap();
    }
}
