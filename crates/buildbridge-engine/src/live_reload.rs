//! Shared preflight for Android and iPhone development-server builds.

/// Check the running server before spending time on a native build. The phone still needs
/// its own route to a LAN server; Android loopback is forwarded through ADB when installing.
pub(crate) async fn check_live_reload_server(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    let response = client.get(url).send().await.map_err(|_| {
        "The live reload server could not be reached. Start your project's development server and check its URL before building or installing.".to_string()
    })?;
    if !response.status().is_success() && !response.status().is_redirection() {
        return Err(format!(
            "The live reload server returned HTTP {}. Check its URL and development server before building or installing.",
            response.status().as_u16()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_reload_requires_a_responding_development_server() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        for (status, succeeds) in [(200, true), (302, true), (404, false), (500, false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let result = check_live_reload_server(&url).await;
            server.join().unwrap();
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            if !succeeds {
                assert!(result.unwrap_err().contains(&format!("HTTP {status}")));
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        assert!(
            check_live_reload_server(&url)
                .await
                .unwrap_err()
                .contains("Start your project's development server")
        );
    }
}
