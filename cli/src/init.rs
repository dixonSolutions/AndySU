//! `andysu init`.

use std::path::{Path, PathBuf};

use crate::R;

pub fn init(_dir: &Path, _app: Option<String>, _repo: Option<String>, _keys: bool) -> R<String> {
    let _ = PathBuf::new();
    Err("init is not implemented yet".into())
}
