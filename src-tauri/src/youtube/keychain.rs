//! The refresh token in the macOS Keychain.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::error::AppError;

/// `errSecItemNotFound`
const NOT_FOUND: i32 = -25300;
const ACCOUNT: &str = "youtube-refresh-token";

pub const SERVICE: &str = "com.audiobooks.app";

fn failed(error: security_framework::base::Error) -> AppError {
    AppError::new(format!("Keychain error: {error}"))
}

pub fn load(service: &str) -> Result<Option<String>, AppError> {
    match get_generic_password(service, ACCOUNT) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| AppError::new("Keychain error: the saved YouTube token is unreadable")),
        Err(error) if error.code() == NOT_FOUND => Ok(None),
        Err(error) => Err(failed(error)),
    }
}

pub fn save(service: &str, token: &str) -> Result<(), AppError> {
    set_generic_password(service, ACCOUNT, token.as_bytes()).map_err(failed)
}

/// Removing a token that is not there is fine.
pub fn delete(service: &str) -> Result<(), AppError> {
    match delete_generic_password(service, ACCOUNT) {
        Err(error) if error.code() != NOT_FOUND => Err(failed(error)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes to the real Keychain (under a test name): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_token_is_saved_replaced_and_removed() {
        let service = "com.audiobooks.app.test";
        delete(service).unwrap();
        assert_eq!(load(service).unwrap(), None);
        save(service, "one").unwrap();
        save(service, "two").unwrap();
        assert_eq!(load(service).unwrap().as_deref(), Some("two"));
        delete(service).unwrap();
        delete(service).unwrap();
        assert_eq!(load(service).unwrap(), None);
    }
}
