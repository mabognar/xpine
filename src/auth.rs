use std::net::TcpListener;
use std::io::{Read, Write};
use webbrowser;

/// Starts the OAuth2 flow and returns the Authorization Code
pub fn get_authorization_code(client_id: &str) -> Option<String> {
    let port = 8080;
    let redirect_uri = format!("http://127.0.0.1:{}", port);

    // 1. Build the Google Auth URL
    // access_type=offline is CRITICAL to get a Refresh Token
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
        client_id={}&\
        redirect_uri={}&\
        response_type=code&\
        scope=https://mail.google.com/&\
        access_type=offline&\
        prompt=consent",
        client_id, redirect_uri
    );

    // 2. Open the user's default browser
    println!("Opening browser to authenticate with Google...");
    if webbrowser::open(&auth_url).is_err() {
        println!("Failed to open the browser. Please manually navigate to:");
        println!("{}", auth_url);
    }

    // 3. Start a local server to catch the callback from Google
    let listener = match TcpListener::bind(format!("127.0.0.1:{}", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Could not bind to port {}: {}", port, e);
            return None;
        }
    };

    println!("Waiting for authorization code...");

    // Listen for the incoming redirect request
    for stream in listener.incoming() {
        if let Ok(mut stream) = stream {
            let mut buffer = [0; 2048];
            if stream.read(&mut buffer).is_err() {
                continue;
            }
            let request = String::from_utf8_lossy(&buffer);

            // 4. Parse the HTTP GET request to find the `?code=...` parameter
            if request.starts_with("GET") {
                if let Some(code) = extract_code(&request) {
                    // Send a success page back to the browser so the user knows they can close the tab
                    let response = "HTTP/1.1 200 OK\r\n\r\n\
                    <html><body>\
                    <h1 style=\"font-family: sans-serif; color: #4CAF50;\">Authentication successful!</h1>\
                    <p style=\"font-family: sans-serif;\">You can safely close this tab and return to xpine.</p>\
                    </body></html>";
                    let _ = stream.write_all(response.as_bytes());

                    return Some(code);
                } else if request.contains("error=") {
                    let response = "HTTP/1.1 400 Bad Request\r\n\r\n<html><body><h1>Authentication failed or was denied.</h1></body></html>";
                    let _ = stream.write_all(response.as_bytes());
                    return None;
                }
            }
        }
    }
    None
}

/// Helper to parse the raw HTTP GET request
fn extract_code(request: &str) -> Option<String> {
    // We are looking for the first line, e.g., "GET /?code=4/0AeaY...&scope=... HTTP/1.1"
    let first_line = request.lines().next()?;

    // Find where the code parameter starts
    let start = first_line.find("code=")?;

    // Find where the parameter ends (either at the next '&' or at the space before HTTP/1.1)
    let end = first_line[start..].find('&')
        .unwrap_or_else(|| first_line[start..].find(' ').unwrap_or(first_line.len() - start));

    // Extract the exact value (+5 skips the "code=" part)
    let code = &first_line[start + 5..start + end];
    Some(code.to_string())
}

use serde::Deserialize;
use reqwest::blocking::Client;
use std::collections::HashMap;

/// The structure of Google's JSON response when exchanging a code or refreshing a token.
#[derive(Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: u64, // Usually 3599 seconds (1 hour)
    pub token_type: String, // Usually "Bearer"
    pub scope: String,
    // The refresh token is ONLY returned the very first time the user consents.
    pub refresh_token: Option<String>,
}

/// Exchanges the short-lived authorization code for an Access Token and Refresh Token
pub fn exchange_code_for_token(
    client_id: &str,
    client_secret: &str,
    code: &str,
) -> Result<TokenResponse, reqwest::Error> {
    let client = Client::new();

    // Build the form data required by Google
    let mut params = HashMap::new();
    params.insert("client_id", client_id);
    params.insert("client_secret", client_secret);
    params.insert("code", code);
    params.insert("grant_type", "authorization_code");
    // This MUST match exactly what we used in the get_authorization_code function
    params.insert("redirect_uri", "http://127.0.0.1:8080");

    println!("Exchanging authorization code for tokens...");

    // Make the POST request
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()?;

    // Deserialize the JSON response directly into our TokenResponse struct
    let token_data: TokenResponse = response.json()?;

    Ok(token_data)
}

pub fn authenticate_google_account(client_id: &str, client_secret: &str) -> Option<TokenResponse> {
    // Step 1: Get the code via the browser
    if let Some(code) = get_authorization_code(client_id) {
        // Step 2: Trade the code for tokens
        match exchange_code_for_token(client_id, client_secret, &code) {
            Ok(tokens) => {
                println!("Successfully authenticated!");
                return Some(tokens);
            }
            Err(e) => {
                eprintln!("Failed to exchange token: {}", e);
            }
        }
    }
    None
}

/// Uses a saved Refresh Token to silently fetch a fresh Access Token
// pub fn refresh_access_token(
//     client_id: &str,
//     client_secret: &str,
//     refresh_token: &str,
// ) -> Result<TokenResponse, reqwest::Error> {
//     let client = Client::new();
//
//     let mut params = HashMap::new();
//     params.insert("client_id", client_id);
//     params.insert("client_secret", client_secret);
//     params.insert("refresh_token", refresh_token);
//     params.insert("grant_type", "refresh_token");
//
//     // Make the POST request
//     let response = client
//         .post("https://oauth2.googleapis.com/token")
//         .form(&params)
//         .send()?;
//
//     // Deserialize the JSON response
//     let token_data: TokenResponse = response.json()?;
//
//     Ok(token_data)
// }

pub fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, reqwest::Error> {
    let client = Client::new();

    let mut params = HashMap::new();
    params.insert("client_id", client_id);
    params.insert("client_secret", client_secret);
    params.insert("refresh_token", refresh_token);
    params.insert("grant_type", "refresh_token");

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()?;

    // Check if Google returned a 200 OK status
    if response.status().is_success() {
        let token_data: TokenResponse = response.json()?;
        Ok(token_data)
    } else {
        // If Google rejected it, print the exact reason!
        let error_text = response.text()?;
        eprintln!("Google API Error: {}", error_text);
        panic!("Failed to fetch token from Google. See error above.");
    }
}
