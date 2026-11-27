//! The refresh tokens in the macOS Keychain, one per channel.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::error::AppError;

/// `errSecItemNotFound`
const NOT_FOUND: i32 = -25300;
/// The only token of builds that knew a single account.
pub const LEGACY_ACCOUNT: &str = "youtube-refresh-token";

pub const SERVICE: &str = "com.audiobooks.app";

pub fn account(channel_id: &str) -> String {
    format!("{LEGACY_ACCOUNT}:{channel_id}")
}

pub trait Tokens {
    fn load(&self, account: &str) -> Result<Option<String>, AppError>;
    fn save(&self, account: &str, token: &str) -> Result<(), AppError>;
    /// Removing a token that is not there is fine.
    fn delete(&self, account: &str) -> Result<(), AppError>;
}

/// The Keychain items under one service name.
pub struct Keychain(pub &'static str);

fn failed(error: security_framework::base::Error) -> AppError {
    AppError::new(format!("Keychain error: {error}"))
}

impl Tokens for Keychain {
    fn load(&self, account: &str) -> Result<Option<String>, AppError> {
        match get_generic_password(self.0, account) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
                AppError::new("Keychain error: the saved YouTube token is unreadable")
            }),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(failed(error)),
        }
    }

    fn save(&self, account: &str, token: &str) -> Result<(), AppError> {
        set_generic_password(self.0, account, token.as_bytes()).map_err(failed)
    }

    fn delete(&self, account: &str) -> Result<(), AppError> {
        match delete_generic_password(self.0, account) {
            Err(error) if error.code() != NOT_FOUND => Err(failed(error)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    pub struct Memory(pub RefCell<HashMap<String, String>>);

    impl Tokens for Memory {
        fn load(&self, account: &str) -> Result<Option<String>, AppError> {
            Ok(self.0.borrow().get(account).cloned())
        }

        fn save(&self, account: &str, token: &str) -> Result<(), AppError> {
            self.0.borrow_mut().insert(account.into(), token.into());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), AppError> {
            self.0.borrow_mut().remove(account);
            Ok(())
        }
    }

    /// Writes to the real Keychain (under a test name): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_token_is_saved_replaced_and_removed() {
        let keychain = Keychain("com.audiobooks.app.test");
        let name = account("UCtest");
        keychain.delete(&name).unwrap();
        assert_eq!(keychain.load(&name).unwrap(), None);
        keychain.save(&name, "one").unwrap();
        keychain.save(&name, "two").unwrap();
        assert_eq!(keychain.load(&name).unwrap().as_deref(), Some("two"));
        keychain.delete(&name).unwrap();
        keychain.delete(&name).unwrap();
        assert_eq!(keychain.load(&name).unwrap(), None);
    }
}
