//! The recorder's capture, call detection, encoding, waiting queue and
//! upload, without its window: the tray app and other PNeX desktop apps
//! share them. Two apps share one queue directory and its single-instance
//! lock (`queue::single_instance`): only the holder records and uploads, so
//! a call is never recorded nor sent twice.

pub mod capture;
pub mod detect;
pub mod encode;
pub mod queue;
pub mod upload;
