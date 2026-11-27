//! The user's YouTube account: signing in uploading a video and its subtitles.
//!
//! The OAuth client file is the user's own, outside the repository; the refresh token lives only in the Keychain.

mod auth;
pub mod commands;
mod draft;
#[cfg(test)]
mod fake;
mod form;
mod job;
mod keychain;
mod upload;
