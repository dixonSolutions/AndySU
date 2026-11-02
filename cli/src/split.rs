//! `andysu split app.apk`: a finished APK becomes a frozen host APK plus a payload directory.

use std::path::PathBuf;

use crate::R;

#[derive(clap::Args, Clone, Debug)]
pub struct SplitArgs {
    /// The finished, signed APK.
    pub apk: PathBuf,
    /// Output directory (receives payload/ and host/).
    #[arg(long, short)]
    pub out: PathBuf,
}

#[derive(serde::Serialize, Debug, Default)]
pub struct SplitReport {}

pub fn split(_a: &SplitArgs) -> R<SplitReport> {
    Err("split is not implemented yet".into())
}
