//! The user's YouTube channels: signing in, uploading a video and its subtitles.
//!
//! The OAuth client file is the user's own, outside the repository; the refresh tokens live only in the Keychain.

pub mod accounts;
mod auth;
mod channels;
pub mod commands;
mod draft;
#[cfg(test)]
mod fake;
mod form;
mod job;
mod keychain;
mod upload;
