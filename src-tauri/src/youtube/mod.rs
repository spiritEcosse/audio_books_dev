//! The user's YouTube account: signing in and (later) uploading.
//!
//! The OAuth client file is the user's own, outside the repository; the refresh token lives only in the Keychain.

mod auth;
pub mod commands;
#[cfg(test)]
mod fake;
mod form;
mod job;
mod keychain;
mod upload;
